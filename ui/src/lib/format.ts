// Client-side mirrors of the Rust formatters (src/state.rs fmt_ms/fmt_bytes)
// plus relative-time helpers. Keep the outputs identical to the server's.

export function fmtMs(ms: number): string {
  if (ms < 1_000) return `${ms} ms`;
  if (ms < 10_000) return `${(ms / 1_000).toFixed(1)} s`;
  if (ms < 60_000) return `${Math.floor(ms / 1_000)} s`;
  if (ms < 3_600_000) {
    const m = Math.floor(ms / 60_000);
    const s = Math.floor((ms % 60_000) / 1_000);
    return `${m}m ${String(s).padStart(2, "0")}s`;
  }
  const h = Math.floor(ms / 3_600_000);
  const m = Math.floor((ms % 3_600_000) / 60_000);
  return `${h}h ${String(m).padStart(2, "0")}m`;
}

export function fmtBytes(b: number): string {
  const K = 1024;
  if (b < K) return `${b} B`;
  if (b < K * K) return `${(b / K).toFixed(1)} kB`;
  if (b < K * K * K) return `${(b / (K * K)).toFixed(1)} MB`;
  return `${(b / (K * K * K)).toFixed(1)} GB`;
}

/** `2026-09-14 11:09:35` in the browser's local time (server renders UTC). */
export function fmtDt(ts: number): string {
  const d = new Date(ts * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export function fmtTime(ts: number): string {
  const d = new Date(ts * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** `just now` · `2 min ago` · `3 h ago` · `2026-09-01` past a week. */
export function relTime(ts: number | null | undefined, now = Date.now() / 1000): string {
  if (!ts) return "—";
  const diff = Math.max(0, now - ts);
  if (diff < 45) return "just now";
  if (diff < 3600) return `${Math.round(diff / 60)} min ago`;
  if (diff < 86_400) return `${Math.round(diff / 3600)} h ago`;
  if (diff < 7 * 86_400) return `${Math.round(diff / 86_400)} d ago`;
  return fmtDt(ts).slice(0, 10);
}

export function taskStateDisplay(s: string | null | undefined): string {
  return (
    (s ?? "").replace("TASK_STATE_", "").replace(/^task_state_/, "").toLowerCase().replace(/_/g, "-") || "—"
  );
}

/** Task state bucket: active / waiting (amber) / done / failed. */
export function taskTone(state: string | null | undefined): "ok" | "bad" | "warn" | "" {
  const s = taskStateDisplay(state);
  if (s === "failed" || s === "rejected" || s === "error") return "bad";
  if (s === "input-required") return "warn";
  if (s === "completed" || s === "canceled") return "ok";
  return "";
}

export function isTaskActive(state: string | null | undefined): boolean {
  const s = taskStateDisplay(state);
  return s === "submitted" || s === "working" || s === "input-required" || s === "queued";
}

/** Shorten a peer/task id for display: keep head + tail. */
export function shortId(id: string, head = 8, tail = 4): string {
  if (id.length <= head + tail + 1) return id;
  return `${id.slice(0, head)}…${id.slice(-tail)}`;
}

/** Stable identity color index for chat avatars (hash → c0..c7). */
export function identityIdx(name: string): number {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) | 0;
  return Math.abs(h) % 8;
}
