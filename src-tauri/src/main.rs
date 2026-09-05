#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::process::Command as TokioCommand;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

// Cuántos links resolvemos al mismo tiempo. Más alto = más rápido, pero
// demasiado alto hace que YouTube empiece a devolver errores/throttling.
const MAX_PARALLEL_RESOLVES: usize = 6;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------- Tipos compartidos con el frontend ----------------

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VideoEntry {
    pub index: usize,
    pub id: String,
    pub url: String,
    pub title: String,
    pub duration: Option<f64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct QselProgress {
    resolved: usize,
    added: usize,
    total: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct QselDone {
    total: usize,
    added: usize,
    resolve_errors: Vec<(String, String)>,
    add_failed: Vec<(String, String)>,
}

struct AppState {
    cancel_flag: Arc<AtomicBool>,
}

// ---------------- Helpers de sistema (equivalentes a los del main.py) ----------------

/// Busca IDM (IDMan.exe) en las rutas habituales de Windows, igual que
/// `_find_idm` en la versión Python.
fn find_idm() -> Option<String> {
    let program_files =
        std::env::var("PROGRAMFILES").unwrap_or_else(|_| r"C:\Program Files".to_string());
    let program_files_x86 = std::env::var("PROGRAMFILES(X86)")
        .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
    let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_default();

    let mut candidates = vec![
        format!("{program_files}\\Internet Download Manager\\IDMan.exe"),
        format!("{program_files_x86}\\Internet Download Manager\\IDMan.exe"),
    ];
    if !local_appdata.is_empty() {
        candidates.push(format!(
            "{local_appdata}\\Programs\\Internet Download Manager\\IDMan.exe"
        ));
    }
    for candidate in &candidates {
        if std::path::Path::new(candidate).is_file() {
            return Some(candidate.clone());
        }
    }
    // También lo busca en el PATH.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("IDMan.exe");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// Devuelve el comando para invocar yt-dlp: si hay un yt-dlp.exe al lado
/// del ejecutable de la app lo usa (versión "portable" sin depender de que
/// esté instalado), si no confía en que esté en el PATH del sistema.
fn resolve_yt_dlp_cmd() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("yt-dlp.exe");
            if bundled.is_file() {
                return bundled.to_string_lossy().to_string();
            }
        }
    }
    "yt-dlp".to_string()
}

/// Limpia un título de video para que sirva como nombre de archivo válido
/// en Windows (mismo criterio que `_sanitize_filename` en Python).
fn sanitize_filename(name: &str) -> String {
    const INVALID: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    let cleaned: String = name
        .chars()
        .map(|c| if INVALID.contains(&c) { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    let truncated: String = trimmed.chars().take(150).collect();
    if truncated.is_empty() {
        "video".to_string()
    } else {
        truncated
    }
}

fn ensure_video_extension(filename: &mut String) {
    let lower = filename.to_lowercase();
    let has_ext = [".mp4", ".mkv", ".webm", ".avi", ".mov"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    if !has_ext {
        filename.push_str(".mp4");
    }
}

fn video_url_from_entry(raw: &str) -> String {
    if raw.starts_with("http") {
        raw.to_string()
    } else {
        format!("https://www.youtube.com/watch?v={raw}")
    }
}

/// Si el link tiene `?list=...`, arma la URL de playlist "pura", igual que
/// hacía la versión Python para evitar que yt-dlp tome solo el video
/// puntual cuando el link también trae `&index=`.
fn extract_playlist_url(raw_url: &str) -> Option<String> {
    let parsed = url::Url::parse(raw_url).ok()?;
    let list_id = parsed
        .query_pairs()
        .find(|(key, _)| key == "list")?
        .1
        .to_string();
    Some(format!("https://www.youtube.com/playlist?list={list_id}"))
}

/// Resuelve una URL directa a un archivo que YA tenga video y audio
/// juntos (obligatorio para IDM). Mismo esquema de 3 intentos que
/// `_resolve_single_link` en Python: hasta 480p combinado, cualquier
/// combinado, o el formato 18 (360p) como último recurso.
async fn resolve_single_link(yt_dlp: &str, video_url: &str) -> Result<String, String> {
    let formats = [
        "best[height<=480][acodec!=none][vcodec!=none]",
        "best[acodec!=none][vcodec!=none]",
        "18",
    ];
    for fmt in formats {
        let mut cmd = TokioCommand::new(yt_dlp);
        cmd.args(["-f", fmt, "-g", "--no-warnings", "--no-playlist", video_url])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        if let Ok(output) = cmd.output().await {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = text.lines().find(|l| !l.trim().is_empty()) {
                    return Ok(line.trim().to_string());
                }
            }
        }
    }
    Err(
        "Este video no tiene ningún formato con audio y video juntos en YouTube (pasa con \
         algunos videos nuevos)."
            .to_string(),
    )
}

async fn run_extract(yt_dlp: &str, url: &str) -> Result<serde_json::Value, String> {
    let mut cmd = TokioCommand::new(yt_dlp);
    cmd.args([
        "--flat-playlist",
        "-J",
        "--no-warnings",
        "--ignore-errors",
        url,
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("No se pudo ejecutar yt-dlp ({e}). ¿Está instalado y en el PATH?"))?;

    if output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("yt-dlp no devolvió datos:\n{stderr}"));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("No se pudo interpretar la respuesta de yt-dlp: {e}"))
}

fn entries_from_json(info: &serde_json::Value) -> Vec<VideoEntry> {
    let items: Vec<&serde_json::Value> =
        if let Some(entries) = info.get("entries").and_then(|e| e.as_array()) {
            entries.iter().filter(|e| !e.is_null()).collect()
        } else {
            vec![info]
        };

    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    if id.is_empty() {
                        format!("Video {}", i + 1)
                    } else {
                        id.clone()
                    }
                });
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("webpage_url").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .unwrap_or_else(|| id.clone());
            let duration = item.get("duration").and_then(|v| v.as_f64());
            VideoEntry {
                index: i,
                id,
                url,
                title,
                duration,
            }
        })
        .collect()
}

fn spawn_idm(idm_path: &str, args: &[&str]) -> std::io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(idm_path);
    cmd.args(args);
    if let Some(dir) = std::path::Path::new(idm_path).parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.spawn()
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

// ---------------- Comandos expuestos a React ----------------

#[tauri::command]
async fn load_playlist(url: String) -> Result<Vec<VideoEntry>, String> {
    let yt_dlp = resolve_yt_dlp_cmd();
    let trimmed = url.trim().to_string();
    if trimmed.is_empty() {
        return Err("Pegá un link primero.".to_string());
    }

    let playlist_url = extract_playlist_url(&trimmed);

    let info = if let Some(ref purl) = playlist_url {
        match run_extract(&yt_dlp, purl).await {
            Ok(v) => v,
            Err(_) => run_extract(&yt_dlp, &trimmed).await?,
        }
    } else {
        run_extract(&yt_dlp, &trimmed).await?
    };

    let items = entries_from_json(&info);
    if items.is_empty() {
        return Err("No se encontró ningún video en ese link.".to_string());
    }
    Ok(items)
}

#[tauri::command]
async fn cancel_queue(state: State<'_, AppState>) -> Result<(), String> {
    state.cancel_flag.store(true, Ordering::SeqCst);
    Ok(())
}

/// Resuelve los links marcados y los va agregando a la cola de IDM en
/// paralelo: apenas se resuelve el link de un capítulo, se manda a IDM al
/// toque (en vez de esperar a tener todos los marcados resueltos). Un pool
/// de tareas resuelve varios links a la vez; una tarea aparte va sacando
/// cada link resuelto de un canal interno y lo agrega a IDM uno por uno
/// (con la pausa entre altas que IDM necesita para no descartar ninguna en
/// silencio). Exactamente el mismo esquema que `_queue_selected_idm_worker`
/// en la versión Python.
#[tauri::command]
async fn queue_selected_to_idm(
    app: AppHandle,
    state: State<'_, AppState>,
    entries: Vec<VideoEntry>,
    folder: String,
) -> Result<(), String> {
    let idm = find_idm().ok_or_else(|| {
        "No encontré Internet Download Manager (IDMan.exe).\n\nInstalá IDM y asegurate de que \
         esté instalado en C:\\Program Files\\Internet Download Manager o C:\\Program Files \
         (x86)\\Internet Download Manager."
            .to_string()
    })?;

    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("No se pudo crear la carpeta destino: {e}"))?;

    state.cancel_flag.store(false, Ordering::SeqCst);
    let cancel_flag = state.cancel_flag.clone();

    let yt_dlp = resolve_yt_dlp_cmd();
    let total = entries.len();

    let resolved_count = Arc::new(AtomicUsize::new(0));
    let added_count = Arc::new(AtomicUsize::new(0));
    let resolve_errors: Arc<AsyncMutex<Vec<(String, String)>>> = Arc::new(AsyncMutex::new(Vec::new()));
    let add_failed: Arc<AsyncMutex<Vec<(String, String)>>> = Arc::new(AsyncMutex::new(Vec::new()));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();

    // --- Consumidor: agrega a IDM uno por uno a medida que van llegando ---
    let consumer_idm = idm.clone();
    let consumer_folder = folder.clone();
    let consumer_added = added_count.clone();
    let consumer_add_failed = add_failed.clone();
    let consumer_app = app.clone();
    let consumer_resolved = resolved_count.clone();
    let consumer_handle = tokio::spawn(async move {
        while let Some((title, url)) = rx.recv().await {
            let mut filename = sanitize_filename(&title);
            ensure_video_extension(&mut filename);

            match spawn_idm(
                &consumer_idm,
                &["/d", &url, "/p", &consumer_folder, "/f", &filename, "/a", "/n"],
            ) {
                Ok(_) => {
                    consumer_added.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) => {
                    consumer_add_failed.lock().await.push((title, e.to_string()));
                }
            }

            let _ = consumer_app.emit(
                "qsel_progress",
                QselProgress {
                    resolved: consumer_resolved.load(Ordering::SeqCst),
                    added: consumer_added.load(Ordering::SeqCst),
                    total,
                },
            );

            // Pausa para que IDM llegue a procesar cada alta antes de la
            // siguiente: si le mandamos muchas casi juntas, descarta
            // algunas en silencio.
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
    });

    // --- Resolvedor: hasta MAX_PARALLEL_RESOLVES en simultáneo ---
    let semaphore = Arc::new(Semaphore::new(MAX_PARALLEL_RESOLVES));
    let mut join_set = tokio::task::JoinSet::new();

    for entry in entries {
        if cancel_flag.load(Ordering::SeqCst) {
            break;
        }
        let permit = semaphore.clone();
        let yt_dlp = yt_dlp.clone();
        let tx = tx.clone();
        let resolved_count = resolved_count.clone();
        let added_count = added_count.clone();
        let resolve_errors = resolve_errors.clone();
        let app = app.clone();
        let cancel_flag_task = cancel_flag.clone();

        join_set.spawn(async move {
            let _permit = permit.acquire_owned().await.ok();
            if cancel_flag_task.load(Ordering::SeqCst) {
                return;
            }

            let video_url = video_url_from_entry(&entry.url);
            match resolve_single_link(&yt_dlp, &video_url).await {
                Ok(direct_url) => {
                    let _ = tx.send((entry.title.clone(), direct_url));
                }
                Err(e) => {
                    resolve_errors.lock().await.push((entry.title.clone(), e));
                }
            }

            let n = resolved_count.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = app.emit(
                "qsel_progress",
                QselProgress {
                    resolved: n,
                    added: added_count.load(Ordering::SeqCst),
                    total,
                },
            );
        });
    }

    while join_set.join_next().await.is_some() {}
    drop(tx); // avisa al consumidor que no van a llegar más links
    let _ = consumer_handle.await;

    if cancel_flag.load(Ordering::SeqCst) {
        let _ = app.emit("qsel_cancelled", ());
        return Ok(());
    }

    // Un respiro extra para que IDM termine de registrar la última tanda
    // antes de avisar que terminamos.
    let added_final = added_count.load(Ordering::SeqCst);
    let extra_wait_ms = (1000 + added_final as u64 * 50).min(4000);
    tokio::time::sleep(std::time::Duration::from_millis(extra_wait_ms)).await;

    let _ = app.emit(
        "qsel_done",
        QselDone {
            total,
            added: added_final,
            resolve_errors: resolve_errors.lock().await.clone(),
            add_failed: add_failed.lock().await.clone(),
        },
    );

    Ok(())
}

/// Botón individual "⬇ IDM" de cada fila: resuelve un solo link y abre la
/// ventana "Agregar descarga" de IDM ya con carpeta y nombre cargados
/// (equivalente a `_send_to_idm` en Python).
#[tauri::command]
async fn send_single_to_idm(url: String, title: String, folder: String) -> Result<(), String> {
    let idm = find_idm().ok_or_else(|| {
        "No encontré Internet Download Manager (IDMan.exe).\n\nInstalá IDM y asegurate de que \
         esté instalado en C:\\Program Files\\Internet Download Manager o C:\\Program Files \
         (x86)\\Internet Download Manager."
            .to_string()
    })?;
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("No se pudo crear la carpeta destino: {e}"))?;

    let yt_dlp = resolve_yt_dlp_cmd();
    let video_url = video_url_from_entry(&url);
    let direct_url = resolve_single_link(&yt_dlp, &video_url).await?;

    let mut filename = sanitize_filename(&title);
    ensure_video_extension(&mut filename);

    spawn_idm(&idm, &["/d", &direct_url, "/p", &folder, "/f", &filename, "/n"])
        .map_err(|e| format!("No se pudo abrir IDM: {e}"))?;

    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            cancel_flag: Arc::new(AtomicBool::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            load_playlist,
            queue_selected_to_idm,
            send_single_to_idm,
            cancel_queue
        ])
        .run(tauri::generate_context!())
        .expect("error corriendo la app de Tauri");
}
