import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { VideoEntry, QselProgress, QselDone } from "./types";

const DEFAULT_OUTPUT = "D:\\EDITOR DE VIDEO";

export default function App() {
  const [url, setUrl] = useState("");
  const [entries, setEntries] = useState<VideoEntry[]>([]);
  const [selected, setSelected] = useState<Record<number, boolean>>({});
  const [outputDir, setOutputDir] = useState(DEFAULT_OUTPUT);
  const [status, setStatus] = useState("");
  const [progressText, setProgressText] = useState("");
  const [progressPct, setProgressPct] = useState(0);
  const [isLoading, setIsLoading] = useState(false);
  const [isBusy, setIsBusy] = useState(false);
  const [rowBusy, setRowBusy] = useState<Record<number, boolean>>({});

  // Evita que, si el usuario dispara una segunda tanda antes de que
  // terminen los listeners de la anterior, se pisen entre sí.
  const runIdRef = useRef(0);

  useEffect(() => {
    const unlistenProgress = listen<QselProgress>("qsel_progress", (event) => {
      const { resolved, added, total } = event.payload;
      setProgressText(`Resolviendo ${resolved}/${total} — en cola IDM: ${added}/${total}`);
      setProgressPct(total ? (resolved / total) * 100 : 0);
    });
    const unlistenDone = listen<QselDone>("qsel_done", (event) => {
      const { total, added, resolveErrors, addFailed } = event.payload;
      setIsBusy(false);
      let msg = `${added}/${total} agregados a la cola de IDM.`;
      if (resolveErrors.length) msg += ` (${resolveErrors.length} no se pudieron resolver)`;
      if (addFailed.length) msg += ` (${addFailed.length} no se pudieron agregar)`;
      setProgressText(msg);
      setStatus(msg);
      if (resolveErrors.length) {
        const detail = resolveErrors
          .slice(0, 10)
          .map(([title, err]) => `• ${title}:\n${err}`)
          .join("\n\n");
        window.alert(`${resolveErrors.length} video(s) no se pudieron resolver:\n\n${detail}`);
      }
    });
    const unlistenCancelled = listen("qsel_cancelled", () => {
      setIsBusy(false);
      setProgressText("Cancelado.");
      setStatus("Cancelado.");
    });

    return () => {
      unlistenProgress.then((f) => f());
      unlistenDone.then((f) => f());
      unlistenCancelled.then((f) => f());
    };
  }, []);

  async function handleLoad() {
    if (!url.trim()) {
      window.alert("Pegá un link primero.");
      return;
    }
    setIsLoading(true);
    setStatus("Buscando capítulos...");
    setEntries([]);
    setSelected({});
    try {
      const items = await invoke<VideoEntry[]>("load_playlist", { url: url.trim() });
      setEntries(items);
      const sel: Record<number, boolean> = {};
      items.forEach((it) => (sel[it.index] = true));
      setSelected(sel);
      setStatus(`${items.length} video(s) encontrados.`);
    } catch (e) {
      setStatus("Error al buscar la playlist.");
      window.alert(`No se pudo cargar la playlist:\n${e}`);
    } finally {
      setIsLoading(false);
    }
  }

  function setAll(value: boolean) {
    const sel: Record<number, boolean> = {};
    entries.forEach((it) => (sel[it.index] = value));
    setSelected(sel);
  }

  async function chooseFolder() {
    const picked = await open({ directory: true, defaultPath: outputDir || undefined });
    if (typeof picked === "string") setOutputDir(picked);
  }

  async function queueSelectedToIdm() {
    const chosen = entries.filter((e) => selected[e.index]);
    if (chosen.length === 0) {
      window.alert("Marcá al menos un video.");
      return;
    }
    const myRun = ++runIdRef.current;
    setIsBusy(true);
    setProgressPct(0);
    setProgressText(`Resolviendo 0/${chosen.length} — en cola IDM: 0/${chosen.length}`);
    try {
      await invoke("queue_selected_to_idm", { entries: chosen, folder: outputDir });
    } catch (e) {
      if (myRun === runIdRef.current) {
        setIsBusy(false);
        window.alert(`No se pudo iniciar la cola de IDM:\n${e}`);
      }
    }
  }

  async function sendRowToIdm(entry: VideoEntry) {
    setRowBusy((r) => ({ ...r, [entry.index]: true }));
    try {
      await invoke("send_single_to_idm", {
        url: entry.url,
        title: entry.title,
        folder: outputDir,
      });
    } catch (e) {
      window.alert(`No se pudo abrir IDM para "${entry.title}":\n${e}`);
    } finally {
      setRowBusy((r) => ({ ...r, [entry.index]: false }));
    }
  }

  async function cancel() {
    try {
      await invoke("cancel_queue");
    } catch {
      /* no-op */
    }
  }

  const selectedCount = entries.filter((e) => selected[e.index]).length;

  return (
    <div className="app">
      <header className="header">
        <h1>🎬 MSK Downloader</h1>
        <p>Descargá playlists de YouTube o mandalas directo a IDM</p>
      </header>

      <main className="body">
        <section className="card">
          <h2>1 · Link de la playlist o video</h2>
          <div className="row">
            <input
              className="input"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://www.youtube.com/playlist?list=..."
              disabled={isLoading}
            />
            <button className="btn" onClick={handleLoad} disabled={isLoading}>
              {isLoading ? "Buscando..." : "Buscar capítulos"}
            </button>
          </div>
          {status && <p className="muted status">{status}</p>}
        </section>

        <section className="card grow">
          <h2>2 · Capítulos encontrados{entries.length > 0 ? ` (${selectedCount}/${entries.length} marcados)` : ""}</h2>
          <div className="row toolbar">
            <button className="btn" onClick={() => setAll(true)}>
              Marcar todos
            </button>
            <button className="btn" onClick={() => setAll(false)}>
              Desmarcar todos
            </button>
          </div>
          <div className="list">
            {entries.map((entry, i) => (
              <div key={entry.index} className={`list-row ${i % 2 === 1 ? "alt" : ""}`}>
                <input
                  type="checkbox"
                  checked={!!selected[entry.index]}
                  onChange={(e) =>
                    setSelected((s) => ({ ...s, [entry.index]: e.target.checked }))
                  }
                />
                <span className="list-title">
                  {entry.index + 1}. {entry.title}
                </span>
                {entry.duration != null && (
                  <span className="list-duration">{formatDuration(entry.duration)}</span>
                )}
                <button
                  className="btn btn-sm"
                  onClick={() => sendRowToIdm(entry)}
                  disabled={!!rowBusy[entry.index]}
                >
                  {rowBusy[entry.index] ? "..." : "⬇ IDM"}
                </button>
              </div>
            ))}
            {entries.length === 0 && !isLoading && (
              <p className="muted empty-hint">Todavía no buscaste ningún link.</p>
            )}
          </div>
        </section>

        <section className="card">
          <h2>3 · Carpeta destino</h2>
          <div className="row">
            <input
              className="input"
              value={outputDir}
              onChange={(e) => setOutputDir(e.target.value)}
            />
            <button className="btn" onClick={chooseFolder}>
              Elegir...
            </button>
          </div>
        </section>
      </main>

      <footer className="footer">
        <div className="progress-track">
          <div className="progress-fill" style={{ width: `${progressPct}%` }} />
        </div>
        {progressText && <p className="muted progress-text">{progressText}</p>}

        <div className="row actions">
          <button
            className="btn btn-accent"
            onClick={queueSelectedToIdm}
            disabled={isBusy || entries.length === 0}
          >
            📥 Poner marcados en cola IDM
          </button>
          <div className="spacer" />
          <button className="btn" onClick={cancel} disabled={!isBusy}>
            Cancelar
          </button>
        </div>
      </footer>
    </div>
  );
}

function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) {
    return `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}`;
  }
  return `${m}:${String(sec).padStart(2, "0")}`;
}
