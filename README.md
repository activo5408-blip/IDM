# MSK Downloader — versión Tauri + React

Reescritura de la app en Tauri (backend en Rust) + React, con el mismo
flujo que ya veníamos usando: buscar capítulos de una playlist de YouTube
y mandarlos a la cola de Internet Download Manager, resolviendo y
encolando en paralelo.

## Qué incluye esta versión

- Buscar capítulos de una playlist/video (usa `yt-dlp` por debajo).
- Lista con checkboxes, marcar/desmarcar todos.
- Elegir carpeta destino (con selector nativo de Windows).
- "📥 Poner marcados en cola IDM": resuelve y encola en paralelo, igual
  que la versión Python (barra de progreso combinada).
- Botón individual "⬇ IDM" por capítulo.
- Cancelar.

## Lo que NO está portado todavía

- "Descargar seleccionados" (bajar con ffmpeg desde la app, sin pasar por IDM).
- "Generar links" (ventana aparte con la lista de links).

Si esto anda bien, las sumamos después.

## Cómo compilarla (recomendado: GitHub Actions, sin instalar nada local)

Este repo ya trae `.github/workflows/build.yml`, igual que la versión
Python. Con subir el repo a GitHub alcanza:

1. Creá un repo en GitHub y subí esta carpeta entera (`git init`, `git add .`,
   `git commit`, `git remote add origin ...`, `git push`).
2. Andá a la pestaña **Actions** del repo — el workflow "Build Windows
   Installer" arranca solo con el push a `main` (o corrélo a mano con
   "Run workflow").
3. Cuando termina (unos 10-15 min la primera vez, porque compila Rust de
   cero), en esa misma corrida vas a ver un artifact llamado
   **MSK-Downloader-Windows-Installers** — bajalo, es un `.zip` con el
   instalador `.exe` (NSIS) y el `.msi`.
4. Corré el instalador en tu Windows como cualquier otro programa.

El workflow baja `yt-dlp.exe` solo y lo empaqueta adentro del instalador
(en `resources` del `tauri.conf.json`), así no dependés de tenerlo
instalado aparte ni en el PATH.

## Cómo correrla en modo desarrollo (si preferís compilar en tu máquina)

Necesitás Node.js y Rust (`rustup`, toolchain MSVC) instalados localmente.

```bash
npm install
npm run tauri dev
```

## Cómo generar el instalador a mano (sin GitHub)

```bash
npm install
npm run tauri build
```

El instalador queda en: `src-tauri/target/release/bundle/`

## Estructura

```
src/                    → Frontend (React + TypeScript)
  App.tsx               → Toda la UI e interacción
  App.css               → Estilos (look tipo Windows 11)
  types.ts              → Tipos compartidos con el backend Rust
src-tauri/
  src/main.rs           → Backend: comandos de Tauri (yt-dlp, IDM, cola)
  tauri.conf.json        → Configuración de la ventana y el build
  capabilities/          → Permisos (selector de carpeta, etc.)
```

## Nota sobre esta primera versión

El código de `src-tauri/src/main.rs` está escrito y revisado a mano, pero
no lo pude compilar de punta a punta en este entorno (necesita un Rust
2024 más nuevo del que hay disponible acá). El frontend (React/TS) sí
está 100% validado: compila y bundlea sin errores. La primera corrida del
workflow en GitHub Actions es la que termina de confirmar que el backend
en Rust compila limpio. Si el Action falla, pegame el log del paso "Build
app (frontend + Tauri)" tal cual aparece y lo arreglamos.
