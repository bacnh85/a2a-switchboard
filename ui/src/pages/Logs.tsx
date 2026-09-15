import { useEffect, useMemo, useState } from "preact/hooks";
import { apiGet, qs } from "../lib/api";
import { onSse } from "../lib/sse";
import { fmtBytes, fmtMs, taskStateDisplay, taskTone } from "../lib/format";
import type { LogsPayload, RouteEntry } from "../lib/types";
import { Badge, Btn, EmptyState, Skeleton } from "../components/ui";
import { RouteDetail } from "./Dashboard";

interface Filters {
  src: string;
  dst: string;
  status: string;
  method: string;
  errors: boolean;
  from: string;
  to: string;
}

const EMPTY: Filters = { src: "", dst: "", status: "", method: "", errors: false, from: "", to: "" };

function filtersFromUrl(): Filters {
  const u = new URLSearchParams(location.search);
  return {
    src: u.get("src") ?? "",
    dst: u.get("dst") ?? "",
    status: u.get("status") ?? "",
    method: u.get("method") ?? "",
    errors: u.get("errors") === "1",
    from: u.get("from") ?? "",
    to: u.get("to") ?? "",
  };
}

/** datetime-local value (local time) → unix seconds */
function localToTs(v: string, endOfDay = false): number | null {
  if (!v) return null;
  const d = new Date(v);
  if (isNaN(d.getTime())) return null;
  return Math.floor(d.getTime() / 1000) + (endOfDay && v.length <= 16 ? 86399 : 0);
}

export function Logs() {
  const [filters, setFilters] = useState<Filters>(filtersFromUrl);
  const [entries, setEntries] = useState<RouteEntry[] | null>(null);
  const [cursor, setCursor] = useState<number | null>(null);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState("");
  const [detail, setDetail] = useState<RouteEntry | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);

  const query = useMemo(() => {
    const fromTs = localToTs(filters.from);
    const toTs = localToTs(filters.to, true);
    return {
      src: filters.src,
      dst: filters.dst,
      status: filters.status,
      method: filters.method,
      errors: filters.errors ? "1" : "",
      from: fromTs ? String(fromTs) : "",
      to: toTs ? String(toTs) : "",
    };
  }, [filters]);

  // reflect filters in the URL (shareable)
  useEffect(() => {
    const u = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) if (v) u.set(k, String(v));
    history.replaceState(null, "", `/logs${u.toString() ? `?${u}` : ""}`);
  }, [query]);

  useEffect(() => {
    setEntries(null);
    apiGet<LogsPayload>(`/api/logs${qs({ ...query, n: 500 })}`)
      .then((d) => {
        setEntries(d.entries);
        setCursor(d.next_before_ts);
        setTotal(d.total_matched);
        setError("");
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, [query]);

  // live prepend while on page 1 with no filters beyond errors
  useEffect(() => {
    if (Object.values(query).some((v) => v)) return;
    return onSse("route", (ev) => {
      setEntries((es) => (es ? [ev.entry, ...es].slice(0, 500) : es));
    });
  }, [query]);

  const loadOlder = async () => {
    if (cursor === null || loadingMore) return;
    setLoadingMore(true);
    try {
      const d = await apiGet<LogsPayload>(`/api/logs${qs({ ...query, n: 500, before: cursor })}`);
      setEntries((es) => [...(es ?? []), ...d.entries]);
      setCursor(d.next_before_ts);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingMore(false);
    }
  };

  const set = (patch: Partial<Filters>) => setFilters((f) => ({ ...f, ...patch }));

  const exportUrl = `/logs/export${qs(query)}`;

  return (
    <div>
      <div class="filterbar">
        <input class="input" placeholder="caller…" value={filters.src} onInput={(e) => set({ src: (e.target as HTMLInputElement).value })} style="width:130px" />
        <input class="input" placeholder="destination…" value={filters.dst} onInput={(e) => set({ dst: (e.target as HTMLInputElement).value })} style="width:130px" />
        <input class="input" placeholder="method…" value={filters.method} onInput={(e) => set({ method: (e.target as HTMLInputElement).value })} style="width:130px" />
        <input class="input" placeholder="status…" value={filters.status} onInput={(e) => set({ status: (e.target as HTMLInputElement).value })} style="width:80px" />
        <label class="checkbox">
          <input type="checkbox" checked={filters.errors} onChange={(e) => set({ errors: (e.target as HTMLInputElement).checked })} /> errors only
        </label>
        <label style="font-size:0.8rem;color:var(--muted)">from</label>
        <input class="input" type="datetime-local" value={filters.from} onChange={(e) => set({ from: (e.target as HTMLInputElement).value })} style="width:200px" />
        <label style="font-size:0.8rem;color:var(--muted)">to</label>
        <input class="input" type="datetime-local" value={filters.to} onChange={(e) => set({ to: (e.target as HTMLInputElement).value })} style="width:200px" />
        <Btn variant="ghost" size="sm" onClick={() => setFilters(EMPTY)}>
          Clear
        </Btn>
        <span style="flex:1" />
        <a href={exportUrl}>
          <Btn variant="ghost" size="sm">
            Export JSONL
          </Btn>
        </a>
      </div>

      <p style="margin:0 0 10px;color:var(--faint);font-size:0.8rem">
        {entries ? `${total} matching entries · newest first · click a row for audited detail` : "loading…"}
      </p>

      {error && entries === null ? (
        <EmptyState title="Could not load the audit log" hint={error} />
      ) : !entries ? (
        <Skeleton h={340} />
      ) : entries.length === 0 ? (
        <EmptyState title="No matching entries" hint="Loosen the filters — or the gateway simply hasn't routed anything matching yet." />
      ) : (
        <>
          <div class="table-wrap">
            <table class="data">
              <thead>
                <tr>
                  <th>Time</th>
                  <th>Route</th>
                  <th>Method</th>
                  <th class="hide-sm">Task</th>
                  <th class="num">Status</th>
                  <th class="num hide-sm">Size</th>
                  <th class="num">Latency</th>
                </tr>
              </thead>
              <tbody>
                {entries.map((r, i) => (
                  <tr key={`${r.ts}-${i}`} class={r.status >= 400 ? "row-bad clickable" : "clickable"} onClick={() => setDetail(r)}>
                    <td class="mono" style="white-space:nowrap">
                      {new Date(r.ts * 1000).toLocaleString()}
                    </td>
                    <td class="mono cell-ellip" title={`${r.src} → ${r.dst}`}>
                      <b>{r.src}</b> → <b>{r.dst}</b>
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
          {cursor !== null && (
            <div style="text-align:center;margin-top:14px">
              <Btn onClick={loadOlder} disabled={loadingMore}>
                {loadingMore ? "Loading…" : "Load older entries"}
              </Btn>
            </div>
          )}
        </>
      )}

      {detail && <RouteDetail e={detail} onClose={() => setDetail(null)} />}
    </div>
  );
}
