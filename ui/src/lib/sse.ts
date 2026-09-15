// One multiplexed SSE connection for the whole app (/api/events).
// Components subscribe per event type; a watchdog reconnects a stream that
// stopped delivering (the server pings every 15s, so 45s of silence = stale).

import { signal } from "@preact/signals";

export type SseStatus = "connecting" | "live" | "reconnecting";
export const sseStatus = signal<SseStatus>("connecting");

export type SseEvent =
  | { type: "route"; entry: import("./types").RouteEntry }
  | { type: "peers"; kind: string; name: string }
  | { type: "chat"; message: import("./types").ChatMessage }
  | { type: "chat_typing"; conv: string; src: string };

type Handler = (ev: SseEvent) => void;
const handlers = new Map<SseEvent["type"], Set<Handler>>();

const EVENTS: SseEvent["type"][] = ["route", "peers", "chat", "chat_typing"];
const WATCHDOG_MS = 45_000;

let es: EventSource | null = null;
let watchdog: ReturnType<typeof setTimeout> | undefined;
let started = false;

export function sseStart() {
  if (started) return;
  started = true;
  connect();
  // A suspended laptop can kill the socket silently; retry on focus.
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && (!es || es.readyState === EventSource.CLOSED)) connect();
  });
}

function armWatchdog() {
  clearTimeout(watchdog);
  watchdog = setTimeout(() => {
    es?.close();
    es = null;
    connect();
  }, WATCHDOG_MS);
}

function connect() {
  sseStatus.value = es ? "reconnecting" : "connecting";
  const source = new EventSource("/api/events");
  es = source;
  for (const name of EVENTS) {
    source.addEventListener(name, (e) => {
      armWatchdog();
      deliver(name as SseEvent["type"], (e as MessageEvent).data);
    });
  }
  source.addEventListener("ping", () => armWatchdog());
  source.onopen = () => {
    sseStatus.value = "live";
    armWatchdog();
  };
  source.onerror = () => {
    sseStatus.value = "reconnecting";
    // EventSource retries on its own; if it gave up entirely, retry on focus.
  };
}

function deliver(type: SseEvent["type"], raw: string) {
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return;
  }
  const set = handlers.get(type);
  if (!set) return;
  const ev = { type, ...(data as object) } as SseEvent;
  for (const h of set) h(ev);
}

/** Subscribe to an SSE event type; returns an unsubscribe fn. */
export function onSse<T extends SseEvent["type"]>(
  type: T,
  handler: (ev: Extract<SseEvent, { type: T }>) => void,
): () => void {
  let set = handlers.get(type);
  if (!set) {
    set = new Set();
    handlers.set(type, set);
  }
  set.add(handler as Handler);
  return () => set!.delete(handler as Handler);
}
