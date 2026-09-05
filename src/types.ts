// Tienen que coincidir en forma con las structs #[derive(Serialize)] de Rust
// (src-tauri/src/main.rs), porque viajan tal cual por invoke()/eventos.

export interface VideoEntry {
  index: number;
  id: string;
  url: string;
  title: string;
  duration: number | null;
}

export interface QselProgress {
  resolved: number;
  added: number;
  total: number;
}

export interface QselDone {
  total: number;
  added: number;
  resolveErrors: [string, string][]; // (título, error)
  addFailed: [string, string][]; // (título, error)
}
