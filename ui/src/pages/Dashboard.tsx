import { useEffect, useMemo, useState } from "preact/hooks";
import { apiGet, qs } from "../lib/api";
import { onSse } from "../lib/sse";
import { fmtBytes, fmtMs, taskStateDisplay, taskTone } from "../lib/format";
import type { RouteEntry, Summary } from "../lib/types";
import { Sparkline, TrafficChart } from "../components/charts";
import { Topology } from "../components/Topology";
import { Badge, EmptyState, Icon, JsonView, Skeleton, SlideOver } from "../components/ui";

const WINDOWS = [
  { sec: 3600, label: "1h" },
  { sec: 6 * 3600, label: "6h" },
  { sec: 24 * 3600, label: "24h" },
];

export function Dashboard() {
  const [windowSec, setWindowSec] = useState(3600);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [error, setError] = useState("");
  const [detail, setDetail] = useState<RouteEntry | null>(null);
  const [errorsOnly, setErrorsOnly] = useState(false);

  const load = (w: number) => {
    apiGet<Summary>(`/api/summary${qs({ window: w === 3600 ? "" : `${w / 3600}h` })}`)
      .then((s) => {
        setSummary(s);
        setError("");
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  };

  useEffect(() => {
    load(windowSec);
    const iv = setInterval(() => load(windowSec), 15_000);
    const onVis = () => {
      if (!document.hidden) load(windowSec);
    };
    document.addEventListener("visibilitychange", onVis);
    return () => {
      clearInterval(iv);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [windowSec]);

  // live events: optimistic bumps + fresh recent entries
  useEffect(() => {
    return onSse("route", () => {
      setSummary((s) => (s ? { ...s, routed: s.routed + 1 } : s));
    });
  }, []);
  useEffect(() => {
    return onSse("peers", () => load(windowSec));
  }, [windowSec]);

  const recent = useMemo(
    () => (summary?.recent ?? []).filter((r) => !errorsOnly || r.status >= 400),
    [summary, errorsOnly],
  );

  if (error && !summary) {
    return (
      <EmptyState
        title="Dashboard unavailable"
        hint={error}
        action={
          <button class="btn" onClick={() => load(windowSec)}>
            Retry
          </button>
        }
      />
    );
  }

  if (!summary) {
    return (
      <div>
        <div class="kpis">
          {[0, 1, 2, 3, 4].map((i) => (
            <div class="kpi" key={i}>
              <Skeleton h={30} w={80} />
              <Skeleton h={12} w={120} />
            </div>
          ))}
        </div>
        <Skeleton h={230} />
        <div style="height:22px" />
        <Skeleton h={420} />
      </div>
    );
  }

  const s = summary;
  const routedSeries = s.buckets.map((b) => b.routed);
  const errSeries = s.buckets.map((b) => b.errors);
  const delta = s.routed - s.prev_routed;
  const deltaPct = s.prev_routed > 0 ? Math.round((delta * 100) / s.prev_routed) : null;

  return (
    <div>
      {/* KPI row */}
      <div class="kpis">
        <a class="kpi" href="/logs">
          <span class="kpi-value">
            {s.routed}
            <span class="kpi-unit">/ {fmtWindow(windowSec)}</span>
            {deltaPct !== null && Math.abs(deltaPct) >= 5 && (
              <span class="kpi-delta" style={`color:${delta >= 0 ? "var(--ok)" : "var(--bad)"}`}>
                {delta >= 0 ? "▲" : "▼"} {Math.abs(deltaPct)}%
              </span>
            )}
          </span>
          <span class="kpi-label">routed · {s.rate_per_min}/min</span>
          <span class="kpi-spark">
            <Sparkline values={routedSeries} />
          </span>
        </a>
        <a class="kpi" href="/logs?errors=1">
          <span class={`kpi-value ${s.errors > 0 ? "bad" : ""}`}>{s.errors}</span>
          <span class="kpi-label">errors{routedSeries.length > 0 ? ` · ${s.err_pct}% of traffic` : ""}</span>
          <span class="kpi-spark">
            <Sparkline values={errSeries} tone={s.errors > 0 ? "bad" : undefined} />
          </span>
        </a>
        <div class="kpi">
          <span class="kpi-value">{s.p95 != null ? fmtMs(s.p95) : "—"}</span>
          <span class="kpi-label">p95 latency{s.p50 != null ? ` · p50 ${fmtMs(s.p50)}` : ""}</span>
        </div>
        <a class="kpi" href="/peers">
          <span class={`kpi-value ${s.pending > 0 ? "warn" : ""}`}>{s.pending}</span>
          <span class="kpi-label">pending approval</span>
        </a>
        <a class="kpi" href="/peers">
          <span class="kpi-value">
            {s.peers_up}
            <span class="kpi-unit">/ {s.peers_total}</span>
          </span>
          <span class="kpi-label">peers healthy{s.channels > 0 ? ` · ${s.channels} reverse ⛓` : ""}</span>
        </a>
        {s.input_required > 0 && (
          <a class="kpi" href="/tasks?state=input-required">
            <span class="kpi-value warn">{s.input_required}</span>
            <span class="kpi-label">tasks need input</span>
          </a>
        )}
      </div>

      {/* traffic chart */}
      <div class="section">
        <div class="section-head">
          <h2>Traffic</h2>
          <span class="hint">requests through the gateway</span>
          <span class="spacer" />
          <div class="tabs">
            {WINDOWS.map((w) => (
              <button key={w.sec} class={`tab${windowSec === w.sec ? " active" : ""}`} onClick={() => setWindowSec(w.sec)}>
                {w.label}
              </button>
            ))}
          </div>
        </div>
        <div class="panel" style="padding:10px 8px 4px">
          <TrafficChart buckets={s.buckets} windowSec={windowSec} />
        </div>
      </div>

      {/* topology */}
      <div class="section">
        <div class="section-head">
          <h2>Routing topology</h2>
          <span class="hint">live — requests flow caller → gateway → peer and back</span>
        </div>
        <Topology peers={s.peers} />
      </div>

      {/* flow log */}
      <div class="section">
        <div class="section-head">
          <h2>Communication log</h2>
          <span class="hint">every routed request, live</span>
          <span class="spacer" />
          <label class="checkbox">
            <input type="checkbox" checked={errorsOnly} onChange={(e) => setErrorsOnly((e.target as HTMLInputElement).checked)} />
            errors only
          </label>
          <a href="/logs" class="hint" style="text-decoration:underline">
            Full audit log
          </a>
        </div>
        <div class="panel" style="padding:6px 12px">
          {recent.length === 0 ? (
            <EmptyState
              title="No traffic yet"
              hint={
                <>
                  Agents route through <code>ANY /peer/&#123;name&#125;/</code> — the first call will show up here instantly.
                </>
              }
            />
          ) : (
            <ul class="flowlog">
              {recent.slice(0, 40).map((r, i) => (
                <FlowRow key={`${r.ts}-${i}`} e={r} onOpen={() => setDetail(r)} />
              ))}
            </ul>
          )}
        </div>
      </div>

      {detail && <RouteDetail e={detail} onClose={() => setDetail(null)} />}
    </div>
  );
}

function FlowRow(props: { e: RouteEntry; onOpen: () => void }) {
  const e = props.e;
  const tone = taskTone(e.task_state);
  return (
    <li class={e.status >= 400 ? "row-bad" : ""} onClick={props.onOpen} title="Audited detail">
      <span class="f-time">{new Date(e.ts * 1000).toLocaleTimeString()}</span>
      <span class="f-route">
        <b>{e.src}</b>
        <span class="arrow">→</span>
        <b>{e.dst}</b>
        <span style="color:var(--muted)"> {e.rpc_method ?? e.method}</span>
      </span>
      {e.task_state && (
        <Badge tone={tone} mono>
          {taskStateDisplay(e.task_state)}
        </Badge>
      )}
      <span class="f-metric">
        <span class={`status ${e.status >= 400 ? "bad" : ""}`}>{e.status}</span> · {fmtBytes(e.bytes)} · {fmtMs(e.latency_ms)}
      </span>
    </li>
  );
}

export function RouteDetail(props: { e: RouteEntry; onClose: () => void }) {
  const e = props.e;
  return (
    <SlideOver title={`Request detail · ${e.src} → ${e.dst}`} onClose={props.onClose}>
      <dl class="kv" style="margin-bottom:18px">
        <dt>time</dt>
        <dd>{new Date(e.ts * 1000).toLocaleString()}</dd>
        <dt>caller</dt>
        <dd>{e.src}</dd>
        <dt>destination</dt>
        <dd>{e.dst}</dd>
        <dt>method</dt>
        <dd>{e.rpc_method ?? e.method}</dd>
        <dt>status</dt>
        <dd>
          <span class={`status ${e.status >= 400 ? "bad" : ""}`}>{e.status}</span>
        </dd>
        <dt>payload</dt>
        <dd>{fmtBytes(e.bytes)}</dd>
        <dt>latency</dt>
        <dd>{fmtMs(e.latency_ms)}</dd>
        {e.rpc_id && (
          <>
            <dt>rpc id</dt>
            <dd>{e.rpc_id}</dd>
          </>
        )}
        {e.task_id && (
          <>
            <dt>task id</dt>
            <dd>
              <a href={`/tasks?q=${encodeURIComponent(e.task_id)}`}>{e.task_id}</a>
            </dd>
          </>
        )}
        {e.task_state && (
          <>
            <dt>task state</dt>
            <dd>
              <Badge tone={taskTone(e.task_state)}>{taskStateDisplay(e.task_state)}</Badge>
            </dd>
          </>
        )}
      </dl>
      {e.preview && (
        <>
          <h3 style="margin-bottom:8px">
            <Icon name="logs" size={13} /> Request preview
          </h3>
          <JsonView value={e.preview} max={260} />
        </>
      )}
      {e.resp_preview && (
        <>
          <h3 style="margin:14px 0 8px">
            <Icon name="logs" size={13} /> Response preview
          </h3>
          <JsonView value={e.resp_preview} max={260} />
        </>
      )}
      <p style="margin-top:16px">
        <a href={`/logs?src=${encodeURIComponent(e.src)}&dst=${encodeURIComponent(e.dst)}`}>Find related entries in the full audit log →</a>
      </p>
    </SlideOver>
  );
}

function fmtWindow(sec: number): string {
  if (sec < 7200) return "1h";
  if (sec < 90_000) return `${sec / 3600}h`;
  return "24h";
}
