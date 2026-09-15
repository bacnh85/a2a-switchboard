import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { useLocation } from "wouter-preact";
import { apiGet, apiPost } from "../lib/api";
import { onSse } from "../lib/sse";
import { identityIdx } from "../lib/format";
import { renderMarkdown } from "../lib/md";
import type { ChatMessage, ChatState } from "../lib/types";
import { Badge, Btn, EmptyState, Icon, Modal, Skeleton, toast } from "../components/ui";

const EMOJI = [
  "😀","😅","😂","🤣","😊","😍","🤔","😎","🥳","😭","😡","🤯","👍","👎","👏","🙏",
  "💪","🤝","✅","❌","⚠️","🔥","💡","🚀","🤖","🧠","📋","📌","🎯","⏰","🕒","🔗",
  "📨","📬","🎉","🫡","👀","🛠️","⚙️","🧪","✨","❓","❗","💤","🔄","📡","🛰️","🗂️",
];

const SLASH = [
  { cmd: "/help", desc: "what the gateway agent can do" },
  { cmd: "/peers", desc: "list accepted agents" },
  { cmd: "/rooms", desc: "list your rooms" },
  { cmd: "/whoami", desc: "who the gateway thinks you are" },
];

/** Sorted dm conv id — mirrors Rust dm_conv(). */
function dmConv(a: string, b: string): string {
  return a <= b ? `dm:${a}|${b}` : `dm:${b}|${a}`;
}

/** Sidebar entry for a conversation (built client-side from registry + last). */
interface SideEntry {
  conv: string;
  title: string;
  kind: "dm" | "room" | "gateway";
  last?: { id: number; ts: number; src: string; text: string; status: string };
  members?: string[];
}

function buildSidebar(state: ChatState, me: string): SideEntry[] {
  const entries: SideEntry[] = [];
  if (me) {
    entries.push({ conv: dmConv(me, "gateway"), title: "gateway", kind: "gateway" });
    for (const p of state.peers) {
      if (p.name !== me) entries.push({ conv: dmConv(me, p.name), title: p.name, kind: "dm" });
    }
    for (const h of state.humans) {
      if (h !== me) entries.push({ conv: dmConv(me, h), title: h, kind: "dm" });
    }
  }
  for (const r of state.rooms) {
    entries.push({ conv: `room:${r.id}`, title: r.name, kind: "room", members: r.members });
  }
  const lastById = new Map(state.conversations.map((c) => [c.id, c.last]));
  for (const e of entries) e.last = lastById.get(e.conv);
  entries.sort((a, b) => (b.last?.ts ?? 0) - (a.last?.ts ?? 0));
  return entries;
}

function isAgentConv(state: ChatState, me: string, conv: string): boolean {
  if (conv === dmConv(me, "gateway")) return true;
  if (conv.startsWith("room:")) {
    const r = state.rooms.find((x) => x.id === conv.slice(5));
    return (r?.members ?? []).some((m) => state.peers.some((p) => p.name === m));
  }
  const other = otherParty(conv, me);
  return state.peers.some((p) => p.name === other);
}

/** The name on the other side of a dm conv id. */
function otherParty(conv: string, me: string): string {
  const parts = conv.slice(3).split("|");
  return parts[0] === me ? parts[1] : parts[0];
}

export function Chat() {
  const [wloc] = useLocation();
  const [state, setState] = useState<ChatState | null>(null);
  const [error, setError] = useState("");
  const [as, setAs] = useState("");
  const [active, setActive] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [pending, setPending] = useState<ChatMessage[]>([]);
  const [hasOlder, setHasOlder] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [filter, setFilter] = useState("");
  const [typingFrom, setTypingFrom] = useState<{ src: string; until: number } | null>(null);
  const [waitingAgent, setWaitingAgent] = useState(false);
  const [roomModal, setRoomModal] = useState<null | "create" | { id: string; name: string; members: string[] }>(null);
  const [threadOpen, setThreadOpen] = useState(false);
  const [unread, setUnread] = useState<Record<string, number>>({});

  const msgsRef = useRef<HTMLDivElement>(null);
  const stickBottom = useRef(true);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const typingSentAt = useRef(0);
  const activeRef = useRef("");
  activeRef.current = active;
  const asRef = useRef("");
  asRef.current = as;

  const loadState = () =>
    apiGet<ChatState>("/api/chat/state")
      .then((s) => {
        setState(s);
        setError("");
        if (!as && s.humans.length > 0) setAs(s.humans[0]);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));

  useEffect(() => {
    loadState();
    const iv = setInterval(loadState, 30_000);
    // new humans/peers refresh the identity picker + sidebar (0.7.x bug fixed)
    const off = onSse("peers", () => loadState());
    return () => {
      clearInterval(iv);
      off();
    };
  }, []);

  // resolve ?conv=dm:<name> or ?conv=room:<id> once an identity is known
  useEffect(() => {
    if (!state || active) return;
    const u = new URLSearchParams(location.search);
    const want = u.get("conv");
    if (!want) return;
    let target = want;
    if (want.startsWith("dm:")) {
      const me = as || state.humans[0];
      if (!me) return;
      const other = want.slice(3);
      target = other === "gateway" ? dmConv(me, "gateway") : dmConv(me, other);
    }
    if (target) {
      setActive(target);
      setThreadOpen(true);
    }
  }, [state, wloc, as, active]);

  const loadMessages = (conv: string) => {
    setMessages([]);
    setPending([]);
    apiGet<{ messages: ChatMessage[]; has_older: boolean }>(`/api/chat/messages?conv=${encodeURIComponent(conv)}`)
      .then((d) => {
        setMessages(d.messages);
        setHasOlder(d.has_older);
        requestAnimationFrame(scrollBottom);
      })
      .catch((e) => toast(e instanceof Error ? e.message : "load failed", "bad"));
  };

  useEffect(() => {
    if (active) {
      loadMessages(active);
      setUnread((u) => ({ ...u, [active]: 0 }));
    }
  }, [active]);

  // live incoming messages + typing
  useEffect(() => {
    const offChat = onSse("chat", (ev) => {
      const m = ev.message;
      if (m.conv === activeRef.current) {
        setMessages((ms) => (ms.some((x) => x.id === m.id) ? ms : [...ms, m]));
        setPending((ps) => ps.filter((p) => !(p.text === m.text && p.src === m.src)));
        requestAnimationFrame(() => {
          if (stickBottom.current) scrollBottom();
        });
        setWaitingAgent(false);
      } else if (m.kind === "chat") {
        setUnread((u) => ({ ...u, [m.conv]: (u[m.conv] ?? 0) + 1 }));
      }
    });
    const offTyping = onSse("chat_typing", (ev) => {
      if (ev.conv === activeRef.current && ev.src !== asRef.current) {
        setTypingFrom({ src: ev.src, until: Date.now() + 4000 });
        setTimeout(() => setTypingFrom((t) => (t && t.until <= Date.now() ? null : t)), 4200);
      }
    });
    return () => {
      offChat();
      offTyping();
    };
  }, []);

  const scrollBottom = () => {
    const el = msgsRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  };
  const onScroll = () => {
    const el = msgsRef.current;
    if (el) stickBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  const loadOlder = async () => {
    if (!active || loadingOlder || messages.length === 0) return;
    setLoadingOlder(true);
    const before = messages[0]?.id ?? 0;
    try {
      const d = await apiGet<{ messages: ChatMessage[]; has_older: boolean }>(
        `/api/chat/history?conv=${encodeURIComponent(active)}&before_id=${before}&n=100`,
      );
      const el = msgsRef.current;
      const prevH = el?.scrollHeight ?? 0;
      setMessages((ms) => [...d.messages, ...ms]);
      setHasOlder(d.has_older);
      requestAnimationFrame(() => {
        if (el) el.scrollTop = el.scrollHeight - prevH;
      });
    } catch (e) {
      toast(e instanceof Error ? e.message : "load failed", "bad");
    } finally {
      setLoadingOlder(false);
    }
  };

  const send = async (text: string) => {
    if (!active || !text.trim()) return;
    if (!as) {
      toast("Create a human operator identity in Settings first", "warn");
      return;
    }
    const optimistic: ChatMessage = {
      id: -Date.now(),
      ts: Date.now() / 1000,
      conv: active,
      src: as,
      text,
      kind: "chat",
      status: "ok",
      pending: true,
    };
    setPending((p) => [...p, optimistic]);
    setWaitingAgent(state ? isAgentConv(state, as, active) : false);
    stickBottom.current = true;
    requestAnimationFrame(scrollBottom);
    try {
      const res = await apiPost<{ messages: ChatMessage[] }>("/api/chat/send", { conv: active, as, text });
      setPending((p) => p.filter((x) => x.id !== optimistic.id));
      const ids = new Set(messages.map((m) => m.id));
      const extra = (res.messages ?? []).filter((m) => !ids.has(m.id));
      if (extra.length > 0) setMessages((ms) => [...ms, ...extra]);
      setWaitingAgent(false);
      loadState();
    } catch (e) {
      setPending((p) =>
        p.map((x) => (x.id === optimistic.id ? { ...x, pending: false, status: "err" as const, error: e instanceof Error ? e.message : "failed" } : x)),
      );
      setWaitingAgent(false);
    }
  };

  const onInput = (el: HTMLTextAreaElement) => {
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 130)}px`;
    // real typing signal (throttled), DMs only
    const now = Date.now();
    if (active.startsWith("dm:") && now - typingSentAt.current > 2500 && as) {
      typingSentAt.current = now;
      apiPost("/api/chat/typing", { conv: active, as }).catch(() => {});
    }
  };

  const sidebar = useMemo(() => (state ? buildSidebar(state, as) : []), [state, as]);
  const visible = useMemo(
    () => (filter ? messages.filter((m) => m.text.toLowerCase().includes(filter.toLowerCase())) : messages),
    [messages, filter],
  );

  if (error && !state) return <EmptyState title="Could not load chat" hint={error} action={<button class="btn" onClick={loadState}>Retry</button>} />;
  if (!state) return <Skeleton h={420} />;

  const noIdentity = state.humans.length === 0;
  const activeEntry = sidebar.find((e) => e.conv === active);
  const activeTitle =
    activeEntry?.title ??
    (active.startsWith("room:") ? state.rooms.find((r) => r.id === active.slice(5))?.name ?? "room" : otherParty(active, as || "?"));
  const peerFor = (title: string) => state.peers.find((p) => p.name === title);

  return (
    <div>
      {noIdentity && (
        <div class="sec-banner" style="border-radius:var(--r-md);margin-bottom:12px">
          No human operator identity exists yet — create one in <a href="/settings">Settings → Human operators</a> to send messages.
        </div>
      )}
      <div class={`chat${threadOpen ? " thread-open" : ""}`}>
        {/* ---- conversation list ---- */}
        <div class="chat-list">
          <div class="chat-list-head">
            <h3>Chats</h3>
            <span style="flex:1" />
            <Btn size="sm" onClick={() => setRoomModal("create")}>
              <Icon name="peers" size={12} /> New room
            </Btn>
          </div>
          <div style="padding:8px 10px;border-bottom:1px solid var(--border)">
            <input
              class="input"
              placeholder="Search messages in thread…"
              value={filter}
              onInput={(e) => setFilter((e.target as HTMLInputElement).value)}
            />
          </div>
          <div class="chat-list-scroll">
            {sidebar.length === 0 && (
              <div style="padding:18px;color:var(--muted);font-size:0.84rem">
                No conversations yet. Create an operator identity in <a href="/settings">Settings</a>, then message a peer or the gateway agent.
              </div>
            )}
            {sidebar.map((c) => {
              const unreadN = unread[c.conv] ?? 0;
              const peer = c.kind === "dm" ? peerFor(c.title) : undefined;
              return (
                <div
                  key={c.conv}
                  class={`conv${active === c.conv ? " active" : ""}`}
                  onClick={() => {
                    setActive(c.conv);
                    setFilter("");
                    setThreadOpen(true);
                    stickBottom.current = true;
                  }}
                >
                  <span class="avatar" style={`background:var(--c${identityIdx(c.title)}-bg);color:var(--c${identityIdx(c.title)})`}>
                    {c.kind === "room" ? "#" : c.title.slice(0, 2).toUpperCase()}
                  </span>
                  <div class="conv-body">
                    <div class="conv-title">
                      {c.title}
                      {peer && <span class={`dot ${peer.healthy === true ? "ok" : peer.healthy === false ? "bad" : ""}`} />}
                      {c.kind === "room" && <Badge>{c.members?.length ?? 0}</Badge>}
                    </div>
                    <div class="conv-preview">{c.last ? `${c.last.src === as ? "you" : c.last.src}: ${c.last.text}` : "—"}</div>
                  </div>
                  {unreadN > 0 && <span class="conv-unread">{unreadN > 99 ? "99+" : unreadN}</span>}
                </div>
              );
            })}
          </div>
        </div>

        {/* ---- thread ---- */}
        <div class="chat-thread">
          {active === "" ? (
            <EmptyState title="Pick a conversation" hint="Choose a chat on the left — DMs with agents, the gateway agent, or a room." />
          ) : (
            <>
              <div class="chat-head">
                <button class="btn-icon back-btn" aria-label="Back" onClick={() => setThreadOpen(false)}>
                  <Icon name="back" />
                </button>
                <span class="avatar" style={`background:var(--c${identityIdx(activeTitle)}-bg);color:var(--c${identityIdx(activeTitle)})`}>
                  {activeEntry?.kind === "room" ? "#" : activeTitle.slice(0, 2).toUpperCase()}
                </span>
                <div>
                  <div class="conv-title">{activeTitle}</div>
                  <div style="font-size:0.75rem;color:var(--faint)">
                    {activeEntry?.kind === "room"
                      ? `${activeEntry.members?.length ?? 0} members`
                      : activeEntry?.kind === "gateway"
                        ? "built-in agent — try /help"
                        : waitingAgent
                          ? "waiting for reply…"
                          : typingFrom
                            ? `${typingFrom.src} is typing…`
                            : "direct message"}
                  </div>
                </div>
                <span style="flex:1" />
                {active.startsWith("room:") && (
                  <Btn
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      const r = state.rooms.find((x) => x.id === active.slice(5));
                      if (r) setRoomModal({ id: r.id, name: r.name, members: [...r.members] });
                    }}
                  >
                    <Icon name="peers" size={12} /> Members
                  </Btn>
                )}
              </div>

              <div class="chat-msgs" ref={msgsRef} onScroll={onScroll}>
                {hasOlder && !filter && (
                  <div style="text-align:center;padding:6px 0 12px">
                    <Btn variant="ghost" size="sm" onClick={loadOlder} disabled={loadingOlder}>
                      {loadingOlder ? "Loading…" : "Load earlier messages"}
                    </Btn>
                  </div>
                )}
                {filter && (
                  <div style="text-align:center;padding:4px 0 10px;color:var(--faint);font-size:0.78rem">
                    {visible.length} of {messages.length} loaded messages match — load earlier to search further back
                  </div>
                )}
                {visible.map((m, i) => (
                  <Bubble key={m.id} m={m} me={as} prev={visible[i - 1]} />
                ))}
                {pending.map((m) => (
                  <Bubble key={m.id} m={m} me={as} />
                ))}
                {waitingAgent && (
                  <div style="align-self:flex-start;margin-top:4px" class="typing-bubble" title="waiting for the agent's reply">
                    <i />
                    <i />
                    <i />
                  </div>
                )}
              </div>

              <Composer
                inputRef={inputRef}
                disabled={noIdentity || !as}
                onSend={send}
                onInput={onInput}
                slashActive={active === dmConv(as || "§", "gateway")}
                as={as}
                setAs={setAs}
                humans={state.humans.map((h) => ({ name: h }))}
              />
            </>
          )}
        </div>
      </div>

      {roomModal && <RoomModal state={state} init={roomModal} onClose={() => setRoomModal(null)} onDone={() => { setRoomModal(null); loadState(); }} />}
    </div>
  );
}

/* ---------- bubbles ---------- */

function Bubble(props: { m: ChatMessage; me: string; prev?: ChatMessage }) {
  const { m, me, prev } = props;
  if (m.kind === "system") {
    return (
      <div class="msg sys" style={m.status === "err" ? "color:var(--bad)" : undefined}>
        {m.text}
      </div>
    );
  }
  const own = m.src === me;
  const sameSender = prev && prev.kind === "chat" && prev.src === m.src && m.ts - prev.ts < 300;
  const day = new Date(m.ts * 1000).toDateString();
  const prevDay = prev ? new Date(prev.ts * 1000).toDateString() : null;
  return (
    <>
      {day !== prevDay && (
        <div class="msg day-sep">
          <span style="background:var(--surface-2);padding:2px 10px;border-radius:999px">{day}</span>
        </div>
      )}
      <div class={`msg${own ? " own" : ""}${m.status === "err" ? " err" : ""}`}>
        {!own && !sameSender && (
          <span class="msg-sender" style={`color:var(--c${identityIdx(m.src)})`}>
            {m.src}
          </span>
        )}
        <span class="bubble">{renderMarkdown(m.text)}</span>
        <span class="msg-meta">
          {new Date(m.ts * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
          {own &&
            (m.pending ? (
              <span title="sending…">◌</span>
            ) : m.status === "err" ? (
              <span title={m.error ?? "failed"}>✗</span>
            ) : (
              <svg width="13" height="10" viewBox="0 0 14 10" style="vertical-align:-1px" aria-label="delivered">
                <path d="M1 5.5 4 8.5 9 2" fill="none" stroke="var(--faint)" stroke-width="1.4" stroke-linecap="round" />
              </svg>
            ))}
        </span>
      </div>
    </>
  );
}

/* ---------- composer ---------- */

function Composer(props: {
  inputRef: { current: HTMLTextAreaElement | null };
  disabled: boolean;
  onSend: (text: string) => void;
  onInput: (el: HTMLTextAreaElement) => void;
  slashActive: boolean;
  as: string;
  setAs: (v: string) => void;
  humans: { name: string }[];
}) {
  const [text, setText] = useState("");
  const [emojiOpen, setEmojiOpen] = useState(false);
  const [slashIdx, setSlashIdx] = useState(0);
  const taRef = useRef<HTMLTextAreaElement>(null);

  const slashMatches = props.slashActive && text.startsWith("/") ? SLASH.filter((s) => s.cmd.startsWith(text.split(" ")[0])) : [];

  const submit = () => {
    const t = text.trim();
    if (!t) return;
    props.onSend(t);
    setText("");
    if (taRef.current) taRef.current.style.height = "auto";
  };

  return (
    <div class="chat-composer">
      {emojiOpen && (
        <div class="composer-pop">
          <div class="emoji-grid">
            {EMOJI.map((e) => (
              <button
                key={e}
                onClick={() => {
                  setText((t) => t + e);
                  taRef.current?.focus();
                }}
              >
                {e}
              </button>
            ))}
          </div>
        </div>
      )}
      {slashMatches.length > 0 && (
        <div class="composer-pop">
          {slashMatches.map((s, i) => (
            <div
              key={s.cmd}
              class={`slash-item${i === slashIdx ? " sel" : ""}`}
              onClick={() => {
                setText(`${s.cmd} `);
                taRef.current?.focus();
              }}
            >
              <code>{s.cmd}</code>
              <span>{s.desc}</span>
            </div>
          ))}
        </div>
      )}
      {props.humans.length > 0 && (
        <select class="input" style="width:auto;padding:6px 8px" title="Send as" value={props.as} onChange={(e) => props.setAs((e.target as HTMLSelectElement).value)}>
          {props.humans.map((h) => (
            <option key={h.name} value={h.name}>
              {h.name}
            </option>
          ))}
        </select>
      )}
      <button class="btn-icon" style="border:1px solid var(--border)" title="Emoji" onClick={() => setEmojiOpen(!emojiOpen)}>
        😀
      </button>
      <textarea
        ref={(el) => {
          taRef.current = el;
          props.inputRef.current = el;
        }}
        rows={1}
        placeholder={props.disabled ? "Create an operator identity first…" : "Message…  (Enter to send, Shift+Enter for newline)"}
        disabled={props.disabled}
        value={text}
        onInput={(e) => {
          setText((e.target as HTMLTextAreaElement).value);
          props.onInput(e.target as HTMLTextAreaElement);
        }}
        onKeyDown={(e) => {
          if (slashMatches.length > 0 && ["ArrowDown", "ArrowUp", "Tab"].includes(e.key)) {
            e.preventDefault();
            setSlashIdx((i) => (e.key === "ArrowUp" ? (i + slashMatches.length - 1) % slashMatches.length : (i + 1) % slashMatches.length));
            return;
          }
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            if (slashMatches.length > 0 && text.split(" ")[0] !== SLASH.find((s) => s.cmd === text.split(" ")[0])?.cmd) {
              setText(`${slashMatches[slashIdx]?.cmd ?? text} `);
              return;
            }
            submit();
          }
          if (e.key === "Escape") setEmojiOpen(false);
        }}
      />
      <Btn variant="primary" onClick={submit} disabled={props.disabled || !text.trim()}>
        <Icon name="send" size={13} /> Send
      </Btn>
    </div>
  );
}

/* ---------- room modal ---------- */

function RoomModal(props: { state: ChatState; init: null | "create" | { id: string; name: string; members: string[] }; onClose: () => void; onDone: () => void }) {
  const editing = props.init !== null && props.init !== "create" ? props.init : null;
  const [name, setName] = useState(editing?.name ?? "");
  const [members, setMembers] = useState<string[]>(editing?.members ?? []);
  const [busy, setBusy] = useState(false);

  const candidates = [...props.state.peers.map((p) => ({ name: p.name, kind: "agent" as const })), ...props.state.humans.map((h) => ({ name: h, kind: "human" as const }))];

  const toggle = (n: string) => setMembers((ms) => (ms.includes(n) ? ms.filter((x) => x !== n) : [...ms, n]));

  const create = async () => {
    if (!name.trim()) return;
    setBusy(true);
    try {
      await apiPost("/api/chat/rooms", { name: name.trim(), members, as: props.state.humans[0] });
      toast(`Room “${name.trim()}” created`, "ok");
      props.onDone();
    } catch (e) {
      toast(e instanceof Error ? e.message : "create failed", "bad");
      setBusy(false);
    }
  };

  const saveMembers = async () => {
    if (!editing) return;
    setBusy(true);
    const add = members.filter((m) => !editing.members.includes(m));
    const remove = editing.members.filter((m) => !members.includes(m));
    try {
      await apiPost(`/api/chat/rooms/${editing.id}/members`, { add, remove });
      toast("Roster updated — agents were notified", "ok");
      props.onDone();
    } catch (e) {
      toast(e instanceof Error ? e.message : "update failed", "bad");
      setBusy(false);
    }
  };

  const del = async () => {
    if (!editing || !confirm(`Delete room “${editing.name}”?`)) return;
    setBusy(true);
    try {
      await apiPost(`/api/chat/rooms/${editing.id}/delete`);
      toast(`Deleted ${editing.name}`);
      props.onDone();
    } catch (e) {
      toast(e instanceof Error ? e.message : "delete failed", "bad");
      setBusy(false);
    }
  };

  return (
    <Modal
      title={editing ? `Room · ${editing.name}` : "New room"}
      onClose={props.onClose}
      footer={
        <>
          {editing && (
            <Btn variant="danger" onClick={del} disabled={busy} style="margin-right:auto">
              Delete room
            </Btn>
          )}
          <Btn variant="ghost" onClick={props.onClose}>
            Cancel
          </Btn>
          <Btn variant="primary" disabled={busy || (!editing && !name.trim())} onClick={editing ? saveMembers : create}>
            {editing ? "Save roster" : "Create room"}
          </Btn>
        </>
      }
    >
      {!editing && (
        <div class="field">
          <label>Room name</label>
          <input class="input" placeholder="e.g. incident-42" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        </div>
      )}
      <div class="field">
        <label>Members {editing && "(changes notify agent members)"}</label>
        {candidates.length === 0 ? (
          <span style="color:var(--muted);font-size:0.84rem">No accepted agents or operators to add yet.</span>
        ) : (
          <div style="display:flex;flex-direction:column;gap:6px;max-height:280px;overflow-y:auto">
            {candidates.map((c) => (
              <label key={c.name} class="checkbox">
                <input type="checkbox" checked={members.includes(c.name)} onChange={() => toggle(c.name)} />
                <span class={`avatar`} style={`background:var(--c${identityIdx(c.name)}-bg);color:var(--c${identityIdx(c.name)});width:22px;height:22px;font-size:0.62rem`}>
                  {c.name.slice(0, 2).toUpperCase()}
                </span>
                <span class="mono" style="font-size:0.84rem">
                  {c.name}
                </span>
                <Badge tone={c.kind === "agent" ? "" : "accent"}>{c.kind}</Badge>
              </label>
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
}
