import { useEffect, useState } from "preact/hooks";
import { apiGet, apiPost, qs } from "../lib/api";
import { onSse } from "../lib/sse";
import { isTaskActive, shortId, taskStateDisplay, taskTone } from "../lib/format";
import type { TaskEntry, TasksPayload } from "../lib/types";
import { Badge, Btn, EmptyState, JsonView, RelTime, SlideOver, Skeleton, toast } from "../components/ui";

const TABS = [
  { v: "active", label: "Active" },
  { v: "input-required", label: "Needs input" },
  { v: "closed", label: "Closed" },
  { v: "", label: "All" },
];

export function Tasks() {
  const initial = new URLSearchParams(location.search);
  const [tab, setTab] = useState(initial.get("state") === "input-required" ? "input-required" : "active");
  const [q, setQ] = useState(initial.get("q") ?? "");
  const [data, setData] = useState<TasksPayload | null>(null);
  const [error, setError] = useState("");
  const [sel, setSel] = useState<TaskEntry | null>(null);

  const stateParam = tab === "active" ? "active" : tab === "closed" ? "closed" : tab === "input-required" ? "input-required" : "";

  const load = () =>
    apiGet<TasksPayload>(`/api/tasks${qs({ state: stateParam, q, n: 300 })}`)
      .then((d) => {
        setData(d);
        setError("");
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));

  useEffect(() => {
    load();
    const iv = setInterval(load, 15_000);
    return () => clearInterval(iv);
  }, [tab, q]);

  // live: a routed call may advance a visible task
  useEffect(() => {
    return onSse("route", () => load());
  }, [tab, q]);

  const patchSel = (t: TaskEntry) => {
    setSel(t);
    setData((d) => (d ? { ...d, tasks: d.tasks.map((x) => (x.id === t.id ? t : x)) } : d));
  };

  if (error && !data) return <EmptyState title="Could not load tasks" hint={error} />;
  if (!data) return <Skeleton h={280} />;

  return (
    <div>
      <div class="filterbar">
        <div class="tabs">
          {TABS.map((t) => (
            <button key={t.v} class={`tab${tab === t.v ? " active" : ""}`} onClick={() => setTab(t.v)}>
              {t.label}
              {t.v === "input-required" && data.tasks.filter((x) => taskStateDisplay(x.state) === "input-required").length > 0 && (
                <span class="badge warn" style="margin-left:6px">
                  {data.tasks.filter((x) => taskStateDisplay(x.state) === "input-required").length}
                </span>
              )}
            </button>
          ))}
        </div>
        <span style="flex:1" />
        <input class="input" placeholder="search id / agent / context…" value={q} onInput={(e) => setQ((e.target as HTMLInputElement).value)} style="width:230px" />
      </div>

      {data.tasks.length === 0 ? (
        <EmptyState
          title={q ? "No tasks match" : tab === "input-required" ? "Nothing waiting on you" : "No tasks in this view"}
          hint={
            q ? undefined : (
              <>
                Tasks appear when agents exchange <code>message/send</code> through the gateway — A2A task lifecycle states are tracked automatically.
              </>
            )
          }
        />
      ) : (
        <div class="table-wrap">
          <table class="data">
            <thead>
              <tr>
                <th>Task</th>
                <th>Agent</th>
                <th class="hide-sm">Caller</th>
                <th>State</th>
                <th class="hide-sm">Updated</th>
                <th class="hide-sm">Latest exchange</th>
              </tr>
            </thead>
            <tbody>
              {data.tasks.map((t) => {
                const tone = taskTone(t.state);
                const needs = taskStateDisplay(t.state) === "input-required";
                return (
                  <tr key={t.id} className={needs ? "row-warn clickable" : "clickable"} onClick={() => setSel(t)}>
                    <td class="mono" title={t.id}>
                      {shortId(t.id, 10, 6)}
                    </td>
                    <td class="mono">
                      <a
                        href={`/peers/${encodeURIComponent(t.dst)}`}
                        onClick={(e) => e.stopPropagation()}
                      >
                        {t.dst}
                      </a>
                    </td>
                    <td class="mono hide-sm">{t.src}</td>
                    <td>
                      <Badge tone={tone}>{taskStateDisplay(t.state)}</Badge>
                    </td>
                    <td class="mono hide-sm" style="white-space:nowrap">
                      <RelTime ts={t.updated_ts} />
                    </td>
                    <td class="mono hide-sm cell-ellip" style="max-width:260px;color:var(--muted)">
                      {t.response_preview ?? t.request_preview ?? "—"}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {sel && <TaskDetail t={sel} onClose={() => setSel(null)} onUpdated={patchSel} />}
    </div>
  );
}

function TaskDetail(props: { t: TaskEntry; onClose: () => void; onUpdated: (t: TaskEntry) => void }) {
  const t = props.t;
  const [reply, setReply] = useState("");
  const [as, setAs] = useState("");
  const [humans, setHumans] = useState<{ name: string }[]>([]);
  const [busy, setBusy] = useState(false);
  const active = isTaskActive(t.state);

  useEffect(() => {
    apiGet<{ humans: { name: string }[] }>("/api/chat/state").then((s) => {
      setHumans(s.humans);
      if (s.humans.length > 0) setAs((a) => a || s.humans[0].name);
    });
  }, []);

  const send = async (e?: Event) => {
    e?.preventDefault();
    if (!reply.trim() || busy) return;
    setBusy(true);
    try {
      const res = await apiPost<{ task: TaskEntry }>(`/api/tasks/${encodeURIComponent(t.id)}/reply`, { as, text: reply.trim() });
      props.onUpdated(res.task ?? { ...t });
      setReply("");
      toast("Reply sent to the agent", "ok");
    } catch (e2) {
      toast(e2 instanceof Error ? e2.message : "reply failed", "bad");
    } finally {
      setBusy(false);
    }
  };

  const cancel = async () => {
    setBusy(true);
    try {
      const res = await apiPost<{ task: TaskEntry }>(`/api/tasks/${encodeURIComponent(t.id)}/cancel`);
      props.onUpdated(res.task ?? { ...t });
      toast("Cancel request sent");
    } catch (e2) {
      toast(e2 instanceof Error ? e2.message : "cancel failed", "bad");
    } finally {
      setBusy(false);
    }
  };

  return (
    <SlideOver title={`Task ${shortId(t.id, 12, 8)}`} onClose={props.onClose}>
      <dl class="kv" style="margin-bottom:18px">
        <dt>task id</dt>
        <dd>{t.id}</dd>
        {t.context_id && (
          <>
            <dt>context id</dt>
            <dd>{t.context_id}</dd>
          </>
        )}
        <dt>agent</dt>
        <dd>
          <a href={`/peers/${encodeURIComponent(t.dst)}`}>{t.dst}</a>
        </dd>
        <dt>caller</dt>
        <dd>{t.src}</dd>
        <dt>state</dt>
        <dd>
          <Badge tone={taskTone(t.state)}>{taskStateDisplay(t.state)}</Badge>
        </dd>
        <dt>opened</dt>
        <dd>
          <RelTime ts={t.created_ts} />
        </dd>
      </dl>

      <h3 style="margin-bottom:10px">Lifecycle</h3>
      <ul class="timeline" style="margin-bottom:20px">
        {t.history.map((h, i) => {
          const tone = taskTone(h.state);
          return (
            <li key={i} class={tone === "ok" ? "t-ok" : tone === "bad" ? "t-bad" : tone === "warn" ? "t-warn" : ""}>
              <div class="t-head">
                {taskStateDisplay(h.state)}
                <span class="t-time">{new Date(h.ts * 1000).toLocaleString()}</span>
              </div>
            </li>
          );
        })}
      </ul>

      {(t.request_preview || t.response_preview) && (
        <>
          <h3 style="margin-bottom:8px">Latest exchange</h3>
          {t.request_preview && <JsonView value={t.request_preview} max={200} />}
          {t.response_preview && (
            <div style="height:8px" />
          )}
          {t.response_preview && <JsonView value={t.response_preview} max={200} />}
          <div style="height:16px" />
        </>
      )}

      {active ? (
        <>
          <h3 style="margin-bottom:8px">Operator intervention</h3>
          {taskStateDisplay(t.state) === "input-required" && (
            <p style="margin:0 0 10px;color:var(--warn);font-size:0.84rem">
              The agent asked for input — answer below and the gateway will deliver it as a follow-up <span class="mono">message/send</span> on this task's context.
            </p>
          )}
          <form onSubmit={send} style="margin-bottom:14px">
            <div class="field-row" style="margin-bottom:8px">
              <div class="field" style="max-width:180px">
                <label>Reply as</label>
                <select class="input" value={as} onChange={(e) => setAs((e.target as HTMLSelectElement).value)}>
                  {humans.length === 0 && <option value="">gateway</option>}
                  {humans.map((h) => (
                    <option key={h.name} value={h.name}>
                      {h.name}
                    </option>
                  ))}
                </select>
              </div>
            </div>
            <div class="field">
              <textarea class="input" rows={3} placeholder="Message to the agent…" value={reply} onInput={(e) => setReply((e.target as HTMLTextAreaElement).value)} />
            </div>
            <div style="display:flex;gap:8px">
              <Btn variant="primary" type="submit" disabled={busy || !reply.trim() || !as}>
                Send reply
              </Btn>
              <Btn variant="danger" onClick={cancel} disabled={busy}>
                Cancel task
              </Btn>
            </div>
          </form>
        </>
      ) : (
        <p style="color:var(--muted);font-size:0.84rem">
          This task is closed ({taskStateDisplay(t.state)}) — no further intervention possible. New messages to the same agent start a fresh task.
        </p>
      )}
    </SlideOver>
  );
}
