import { type ComponentChildren, type JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { signal } from "@preact/signals";
import { relTime } from "../lib/format";
import { sseStatus } from "../lib/sse";

/* ---------- icons (inline, stroke-based, 16px grid) ---------- */

const PATHS: Record<string, ComponentChildren> = {
  dashboard: (
    <>
      <rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1" />
      <rect x="9" y="2.5" width="4.5" height="4.5" rx="1" />
      <rect x="2.5" y="9" width="4.5" height="4.5" rx="1" />
      <rect x="9" y="9" width="4.5" height="4.5" rx="1" />
    </>
  ),
  peers: (
    <>
      <circle cx="8" cy="4.5" r="2" />
      <circle cx="3" cy="12" r="2" />
      <circle cx="13" cy="12" r="2" />
      <path d="M6.8 5.8 4 10.2M9.2 5.8 12 10.2M5 12h6" />
    </>
  ),
  logs: (
    <>
      <rect x="3" y="2.5" width="10" height="11" rx="1.5" />
      <path d="M5.5 5.5h5M5.5 8h5M5.5 10.5h3" />
    </>
  ),
  chat: (
    <>
      <path d="M13.5 8a5.5 4.5 0 0 1-8.1 4L2.5 13l1-2.8A4.6 4.6 0 0 1 2.5 8 5.5 4.5 0 0 1 13.5 8z" />
    </>
  ),
  tasks: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 5.5V8l1.8 1.5" />
    </>
  ),
  settings: (
    <>
      <circle cx="8" cy="8" r="2" />
      <path d="M8 2v2M8 12v2M2 8h2M12 8h2M3.8 3.8l1.4 1.4M10.8 10.8l1.4 1.4M12.2 3.8l-1.4 1.4M5.2 10.8l-1.4 1.4" />
    </>
  ),
  bell: (
    <>
      <path d="M8 2.5a4 4 0 0 1 4 4c0 3 .8 4 1.5 4.7H2.5C3.2 10.5 4 9.5 4 6.5a4 4 0 0 1 4-4z" />
      <path d="M6.7 13.5a1.4 1.4 0 0 0 2.6 0" />
    </>
  ),
  sun: (
    <>
      <circle cx="8" cy="8" r="3" />
      <path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1 1M11.6 11.6l1 1M12.6 3.4l-1 1M4.4 11.6l-1 1" />
    </>
  ),
  moon: <path d="M13.5 9.5A6 6 0 0 1 6.5 2.5a6 6 0 1 0 7 7z" />,
  menu: <path d="M2.5 4h11M2.5 8h11M2.5 12h11" />,
  close: <path d="M4 4l8 8M12 4l-8 8" />,
  copy: (
    <>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
      <path d="M10.5 3.5h-7a1 1 0 0 0-1 1v7" />
    </>
  ),
  check: <path d="M3 8.5 6.5 12 13 4.5" />,
  send: <path d="M14 2 7 9M14 2 9.5 14 7 9 2 6.5 14 2z" />,
  back: <path d="M10 3 5 8l5 5" />,
  external: (
    <>
      <path d="M6.5 3.5H3.5a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-3" />
      <path d="M9.5 2.5h4v4M13 3 7.5 8.5" />
    </>
  ),
  search: (
    <>
      <circle cx="7" cy="7" r="4" />
      <path d="M10 10l3.5 3.5" />
    </>
  ),
};

export function Icon(props: { name: string; size?: number; class?: string }) {
  return (
    <svg
      width={props.size ?? 16}
      height={props.size ?? 16}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linecap="round"
      stroke-linejoin="round"
      class={props.class}
      aria-hidden="true"
    >
      {PATHS[props.name]}
    </svg>
  );
}

export function BrandMark({ size = 22 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
      <path d="M16 4l10 6v12l-10 6-10-6V10l10-6z" fill="none" stroke="var(--accent)" stroke-width="2.2" />
      <circle cx="16" cy="16" r="2.6" fill="var(--accent)" />
      <circle cx="16" cy="6.8" r="1.7" fill="currentColor" />
      <circle cx="24.4" cy="20.6" r="1.7" fill="currentColor" />
      <circle cx="7.6" cy="20.6" r="1.7" fill="currentColor" />
      <path d="M16 9.8v3.6M14.2 17.4l-4.8 2.6M17.8 17.4l4.8 2.6" stroke="var(--accent)" stroke-width="1.4" />
    </svg>
  );
}

/* ---------- primitives ---------- */

type BtnProps = {
  variant?: "default" | "primary" | "danger" | "ghost" | "icon";
  size?: "md" | "sm";
} & Omit<JSX.IntrinsicElements["button"], "size">;

export function Btn({ variant = "default", size = "md", class: cls = "", ...rest }: BtnProps) {
  if (variant === "icon") return <button class={`btn-icon ${cls}`} {...rest} />;
  const v = variant === "default" ? "" : ` btn-${variant}`;
  return <button class={`btn${v}${size === "sm" ? " btn-sm" : ""} ${cls}`} {...rest} />;
}

export function Badge(props: { tone?: "" | "ok" | "bad" | "warn" | "accent"; mono?: boolean; title?: string; children: ComponentChildren }) {
  const t = props.tone ? ` ${props.tone}` : "";
  return (
    <span class={`badge${t}${props.mono ? " mono" : ""}`} title={props.title}>
      {props.children}
    </span>
  );
}

export function Dot(props: { tone: "ok" | "bad" | "warn" | "" | "pulse" }) {
  return <span class={`dot ${props.tone}`} />;
}

export function Conn() {
  const s = sseStatus.value;
  const [label, tone] =
    s === "live" ? (["live", "ok"] as const) : s === "connecting" ? (["connecting", "warn"] as const) : (["reconnecting", "bad"] as const);
  return (
    <span class="conn" title={`Live events: ${label}`}>
      <Dot tone={tone} />
      {label}
    </span>
  );
}

export function EmptyState(props: { title: string; hint?: ComponentChildren; action?: ComponentChildren }) {
  return (
    <div class="empty">
      <div class="empty-title">{props.title}</div>
      {props.hint && <div class="empty-hint">{props.hint}</div>}
      {props.action && <div class="empty-action">{props.action}</div>}
    </div>
  );
}

export function Skeleton(props: { h?: number; w?: number | string }) {
  return <div class="skeleton" style={{ height: `${props.h ?? 14}px`, width: typeof props.w === "number" ? `${props.w}px` : (props.w ?? "100%") }} />;
}

/** Relative time that re-renders on a shared 60s tick. */
const tick = signal(Date.now());
setInterval(() => (tick.value = Date.now()), 60_000);

export function RelTime(props: { ts: number | null | undefined }) {
  tick.value;
  return (
    <span class="mono" title={props.ts ? new Date(props.ts * 1000).toLocaleString() : undefined}>
      {relTime(props.ts)}
    </span>
  );
}

export function CopyBtn(props: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = props.text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  };
  return (
    <Btn variant="ghost" size="sm" onClick={copy} title="Copy to clipboard">
      <Icon name={copied ? "check" : "copy"} size={13} />
      {props.label && (copied ? "Copied" : props.label)}
    </Btn>
  );
}

/** Masked token / secret with reveal + copy. */
export function TokenRow(props: { token: string; reveal?: boolean }) {
  const [shown, setShown] = useState(props.reveal ?? false);
  const masked = "•".repeat(12) + props.token.slice(-4);
  return (
    <span class="token-row">
      <span class={`token-val${shown ? " revealed" : ""}`}>{shown ? props.token : masked}</span>
      <Btn variant="ghost" size="sm" onClick={() => setShown(!shown)}>
        {shown ? "Hide" : "Reveal"}
      </Btn>
      <CopyBtn text={props.token} />
    </span>
  );
}

/* ---------- overlays ---------- */

export function SlideOver(props: { title: ComponentChildren; onClose: () => void; children: ComponentChildren }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [props.onClose]);
  return (
    <>
      <div class="scrim" onClick={props.onClose} />
      <aside class="slideover" role="dialog" aria-modal="true">
        <div class="slideover-head">
          <h2>{props.title}</h2>
          <span style="flex:1" />
          <Btn variant="icon" onClick={props.onClose} aria-label="Close">
            <Icon name="close" />
          </Btn>
        </div>
        <div class="slideover-body">{props.children}</div>
      </aside>
    </>
  );
}

export function Modal(props: { title: string; onClose: () => void; footer?: ComponentChildren; children: ComponentChildren }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [props.onClose]);
  return (
    <>
      <div class="scrim" onClick={props.onClose} />
      <div class="modal" role="dialog" aria-modal="true" aria-label={props.title}>
        <div class="modal-head">
          <h2>{props.title}</h2>
          <span style="flex:1" />
          <Btn variant="icon" onClick={props.onClose} aria-label="Close">
            <Icon name="close" />
          </Btn>
        </div>
        <div class="modal-body">{props.children}</div>
        {props.footer && <div class="modal-foot">{props.footer}</div>}
      </div>
    </>
  );
}

/* ---------- toasts ---------- */

export interface Toast {
  id: number;
  text: string;
  tone: "" | "ok" | "bad" | "warn";
}
export const toasts = signal<Toast[]>([]);
let toastSeq = 0;

export function toast(text: string, tone: Toast["tone"] = "") {
  const id = ++toastSeq;
  toasts.value = [...toasts.value, { id, text, tone }];
  setTimeout(() => {
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }, 4200);
}

export function Toasts() {
  return (
    <div class="toasts">
      {toasts.value.map((t) => (
        <div key={t.id} class={`toast ${t.tone}`}>
          {t.text}
        </div>
      ))}
    </div>
  );
}

/* ---------- json viewer ---------- */

function highlightJson(json: string): ComponentChildren[] {
  // Tiny tokenizer — strings/keys/numbers/bools/null; content is plain text so
  // the output stays XSS-safe (no dangerouslySetInnerHTML).
  const out: ComponentChildren[] = [];
  const re = /("(?:[^"\\]|\\.)*")(\s*:)?|(\b(?:true|false|null)\b)|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  while ((m = re.exec(json))) {
    if (m.index > last) out.push(json.slice(last, m.index));
    if (m[1] && m[2]) {
      out.push(
        <span key={k++} class="j-key">
          {m[1]}
        </span>,
        m[2],
      );
    } else if (m[1]) {
      out.push(
        <span key={k++} class="j-str">
          {m[1]}
        </span>,
      );
    } else if (m[3]) {
      out.push(
        <span key={k++} class="j-bool">
          {m[3]}
        </span>,
      );
    } else if (m[4]) {
      out.push(
        <span key={k++} class="j-num">
          {m[4]}
        </span>,
      );
    }
    last = re.lastIndex;
  }
  out.push(json.slice(last));
  return out;
}

export function JsonView(props: { value: unknown; max?: number }) {
  let text: string;
  try {
    text = typeof props.value === "string" ? props.value : JSON.stringify(props.value, null, 2);
  } catch {
    text = String(props.value);
  }
  const ref = useRef<HTMLPreElement>(null);
  return (
    <pre class="json-view" ref={ref} style={props.max ? `max-height:${props.max}px` : undefined}>
      {highlightJson(text)}
    </pre>
  );
}
