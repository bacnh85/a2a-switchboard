import { useEffect, useState } from "preact/hooks";
import { useParams } from "wouter-preact";
import { apiGet, qs } from "../lib/api";
import { onSse } from "../lib/sse";
import { fmtBytes, fmtMs, relTime, taskStateDisplay, taskTone, identityIdx } from "../lib/format";
import type { PeerDetail as Detail } from "../lib/types";
import { Badge, Btn, Dot, EmptyState, JsonView, Skeleton } from "../components/ui";
import { RouteDetail } from "./Dashboard";
import type { RouteEntry } from "../lib/types";

export function PeerDetail() {
  const params = useParams();
  const name = params.name ?? "";
  const [dir, setDir] = useState<"" | "in" | "out">("");
  const [data, setData] = useState<Detail | null>(null);
  const [error, setError] = useState("");
  const [showRaw, setShowRaw] = useState(false);
  const [detail, setDetail] = useState<RouteEntry | null>(null);

  const load = () =>
    apiGet<Detail>(`/api/peers/${encodeURIComponent(name)}${qs({ dir })}`)
      .then((d) => {
        setData(d);
        setError("");
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));

  useEffect(() => {
    load();
    const iv = setInterval(load, 30_000);
    const off = onSse("peers", () => load());
    return () => {
      clearInterval(iv);
      off();
    };
  }, [name, dir]);

  if (error) {
    return (
      <EmptyState
        title={`Could not load ${name}`}
        hint={error}
        action={
          <a href="/peers">
            <button class="btn">Back to peers</button>
          </a>
        }
      />
    );
  }
  if (!data) return <Skeleton h={300} />;

  const p = data.peer;
  const human = p.kind === "human";
  const health = p.healthy === true ? "ok" : p.healthy === false ? "bad" : "";
  const card = data.card;

  return (
    <div>
      {/* header */}
      <div class="section" style="display:flex;align-items:center;gap:12px;flex-wrap:wrap">
        <span class="avatar" style={`background:var(--c${identityIdx(p.name)}-bg);color:var(--c${identityIdx(p.name)});width:38px;height:38px;font-size:0.95rem`}>
          {p.name.slice(0, 2).toUpperCase()}
        </span>
        <div>
          <h2 style="font-size:1.2rem" class="mono">
            {p.name} {human && <Badge tone="accent">operator</Badge>}
          </h2>
          <div style="display:flex;gap:8px;align-items:center;margin-top:3px;flex-wrap:wrap">
            <Badge tone={p.state === "accepted" ? "ok" : p.state === "pending" ? "warn" : ""}>{p.state}</Badge>
            <span style="display:inline-flex;align-items:center;gap:5px">
              <Dot tone={health} />
              <span style="font-size:0.8rem;color:var(--muted)">{p.healthy === true ? "healthy" : p.healthy === false ? "unreachable" : "unknown"}</span>
            </span>
            {data.channel && <Badge mono title="firewalled peer on a reverse channel">⛓ reverse channel</Badge>}
            <span style="font-size:0.8rem;color:var(--muted)" class="mono">
              {data.ok_count + data.err_count > 0 ? `${data.ok_count + data.err_count} routed · ${data.err_count} err` : "no traffic yet"}
            </span>
          </div>
        </div>
        <span style="flex:1" />
        {!human && p.state === "accepted" && (
          <a href={`/chat?conv=dm:${encodeURIComponent(p.name)}`}>
            <Btn>Chat</Btn>
          </a>
        )}
      </div>

      {/* identity */}
      <div class="section">
        <div class="section-head">
          <h2>Identity</h2>
        </div>
        <div class="panel">
          <dl class="kv">
            <dt>URL</dt>
            <dd>{p.url && !p.url.startsWith("local://") ? p.url : "—"}</dd>
            <dt>registered</dt>
            <dd title={new Date(p.registered_at * 1000).toLocaleString()}>{relTime(p.registered_at)}</dd>
            <dt>last seen</dt>
            <dd>
              <span style={!p.last_seen ? "" : ""}>{relTime(p.last_seen)}</span>
            </dd>
            <dt>last IP</dt>
            <dd>{p.last_ip ?? "—"}</dd>
            <dt>registered from</dt>
            <dd>{p.reg_ip ?? "—"}</dd>
            <dt>admission</dt>
            <dd class="plain">{p.auto_accepted ? "auto (bootstrap token)" : "manual approval"}</dd>
          </dl>
        </div>
      </div>

      {/* agent card */}
      {card && (
        <div class="section">
          <div class="section-head">
            <h2>Agent card</h2>
            <span class="hint">declared by the peer at registration</span>
            <span class="spacer" />
            <Btn variant="ghost" size="sm" onClick={() => setShowRaw(!showRaw)}>
              {showRaw ? "Structured" : "Raw JSON"}
            </Btn>
          </div>
          <div class="panel">
            {showRaw ? (
              <JsonView value={card.raw} />
            ) : (
              <>
                <div style="display:flex;gap:14px;align-items:baseline;flex-wrap:wrap;margin-bottom:6px">
                  <h3 style="font-size:1rem">{card.name || p.name}</h3>
                  {card.version && <Badge mono>v{card.version}</Badge>}
                  {card.provider && <span style="color:var(--muted);font-size:0.82rem">{card.provider}</span>}
                </div>
                {card.description && <p style="margin:0 0 10px;color:var(--muted);font-size:0.88rem">{card.description}</p>}
                <div style="display:flex;gap:6px;flex-wrap:wrap;margin-bottom:12px">
                  {card.streaming && <Badge tone="accent">streaming</Badge>}
                  {card.push && <Badge tone="accent">push notifications</Badge>}
                  {card.sth && <Badge tone="accent">state transition history</Badge>}
                  {!card.streaming && !card.push && !card.sth && <span style="color:var(--faint);font-size:0.8rem">no capabilities declared</span>}
                </div>
                {card.skills.length > 0 && (
                  <table class="data" style="border:1px solid var(--border);border-radius:var(--r-md);overflow:hidden">
                    <thead>
                      <tr>
                        <th>Skill</th>
                        <th class="hide-sm">Tags</th>
                        <th>Description</th>
                      </tr>
                    </thead>
                    <tbody>
                      {card.skills.map((s, i) => (
                        <tr key={i}>
                          <td style="font-weight:600;white-space:nowrap">{s.name || s.id}</td>
                          <td class="mono hide-sm" style="font-size:0.78rem;color:var(--muted)">
                            {s.tags}
                          </td>
                          <td style="color:var(--muted);font-size:0.84rem">{s.description}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </>
            )}
          </div>
        </div>
      )}

      {/* traffic */}
      <div class="section">
        <div class="section-head">
          <h2>Recent traffic</h2>
          <span class="hint">
            {data.traffic_total} total · {data.err_count} errors
          </span>
          <span class="spacer" />
          <div class="tabs">
            {[
              { v: "", label: "All" },
              { v: "in", label: "Calls to peer" },
              { v: "out", label: "Calls by peer" },
            ].map((t) => (
              <button key={t.v} class={`tab${dir === t.v ? " active" : ""}`} onClick={() => setDir(t.v as "" | "in" | "out")}>
                {t.label}
              </button>
            ))}
          </div>
          <a class="hint" style="text-decoration:underline" href={`/logs?${dir === "out" ? "src" : "dst"}=${encodeURIComponent(name)}`}>
            Full log →
          </a>
        </div>
        {data.traffic.length === 0 ? (
          <EmptyState title="No routed traffic" hint="Exchanges through the gateway involving this peer will appear here." />
        ) : (
          <div class="table-wrap">
            <table class="data">
              <thead>
                <tr>
                  <th>Time</th>
                  <th>{dir === "out" ? "Destination" : dir === "in" ? "Caller" : "Route"}</th>
                  <th>Method</th>
                  <th class="hide-sm">Task</th>
                  <th class="num">Status</th>
                  <th class="num hide-sm">Size</th>
                  <th class="num">Latency</th>
                </tr>
              </thead>
              <tbody>
                {data.traffic.map((r, i) => (
                  <tr key={i} class={r.status >= 400 ? "row-bad clickable" : "clickable"} onClick={() => setDetail(r)}>
                    <td class="mono" style="white-space:nowrap">
                      {relTime(r.ts)}
                    </td>
                    <td class="mono">
                      {dir === "" ? (
                        <>
                          <b>{r.src}</b> → <b>{r.dst}</b>
                        </>
                      ) : (
                        <b>{dir === "out" ? r.dst : r.src}</b>
                      )}
                    </td>
                    <td class="mono">{r.rpc_method ?? r.method}</td>
                    <td class="hide-sm">
                      {r.task_state ? <Badge tone={taskTone(r.task_state)}>{taskStateDisplay(r.task_state)}</Badge> : <span style="color:var(--faint)">—</span>}
                    </td>
                    <td class="num">
                      <span class={`status ${r.status >= 400 ? "bad" : ""}`}>{r.status}</span>
                    </td>
                    <td class="num hide-sm">{fmtBytes(r.bytes)}</td>
                    <td class="num">{fmtMs(r.latency_ms)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {detail && <RouteDetail e={detail} onClose={() => setDetail(null)} />}
    </div>
  );
}
