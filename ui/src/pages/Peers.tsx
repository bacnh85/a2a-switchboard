import { useEffect, useState } from "preact/hooks";
import { apiGet, apiPost } from "../lib/api";
import { onSse } from "../lib/sse";
import { identityIdx, relTime } from "../lib/format";
import type { PeerRow, PeersPayload } from "../lib/types";
import { MiniBars } from "../components/charts";
import { Badge, Btn, Dot, EmptyState, Skeleton, toast } from "../components/ui";

export function Peers() {
  const [data, setData] = useState<PeersPayload | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ name: string; action: "revoke" | "delete" } | null>(null);

  const load = () =>
    apiGet<PeersPayload>("/api/peers")
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
  }, []);

  const act = async (name: string, action: "accept" | "reject" | "revoke" | "delete") => {
    setBusy(`${action}:${name}`);
    try {
      await apiPost(`/api/peers/${encodeURIComponent(name)}/${action}`);
      toast(
        action === "accept" ? `Accepted ${name}` : action === "reject" ? `Rejected ${name}` : action === "revoke" ? `Revoked ${name}` : `Deleted ${name}`,
        action === "accept" ? "ok" : "",
      );
      setConfirm(null);
      await load();
    } catch (e) {
      toast(e instanceof Error ? e.message : `${action} failed`, "bad");
    } finally {
      setBusy(null);
    }
  };

  if (error && !data) {
    return (
      <EmptyState
        title="Could not load peers"
        hint={error}
        action={
          <button class="btn" onClick={load}>
            Retry
          </button>
        }
      />
    );
  }
  if (!data) return <Skeleton h={220} />;

  return (
    <div>
      {data.pending.length > 0 && (
        <div class="section">
          <div class="section-head">
            <h2>Pending approval</h2>
            <Badge tone="warn">{data.pending.length}</Badge>
            <span class="hint">registered with the gateway token — waiting for you</span>
          </div>
          <div class="table-wrap">
            <table class="data">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>URL</th>
                  <th class="hide-sm">Registered</th>
                  <th class="hide-sm">From IP</th>
                  <th style="text-align:right">Actions</th>
                </tr>
              </thead>
              <tbody>
                {data.pending.map((p) => (
                  <tr key={p.name} class="row-warn">
                    <td>
                      <a href={`/peers/${encodeURIComponent(p.name)}`} class="mono">
                        {p.name}
                      </a>
                    </td>
                    <td class="mono cell-ellip" title={p.url ?? ""}>
                      {p.url ?? "—"}
                    </td>
                    <td class="mono hide-sm">{relTime(p.registered_at)}</td>
                    <td class="mono hide-sm">{p.reg_ip ?? "—"}</td>
                    <td style="text-align:right;white-space:nowrap">
                      <Btn variant="primary" size="sm" disabled={busy !== null} onClick={() => act(p.name, "accept")}>
                        Accept
                      </Btn>{" "}
                      <Btn variant="danger" size="sm" disabled={busy !== null} onClick={() => act(p.name, "reject")}>
                        Reject
                      </Btn>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <div class="section">
        <div class="section-head">
          <h2>Accepted</h2>
          <Badge>{data.accepted.length}</Badge>
          <span class="spacer" />
          <span class="hint">admitted peers can call and be called</span>
        </div>
        {data.accepted.length === 0 ? (
          <EmptyState
            title="No accepted peers yet"
            hint={
              <>
                Agents register with <code>POST /register</code> using the gateway token (Settings), land in pending, and you accept them here.
              </>
            }
          />
        ) : (
          <div class="table-wrap">
            <table class="data">
              <thead>
                <tr>
                  <th>Name</th>
                  <th class="hide-sm">URL</th>
                  <th class="hide-sm">Health</th>
                  <th class="hide-sm">Activity 1h</th>
                  <th class="hide-sm">Last seen</th>
                  <th style="text-align:right">Actions</th>
                </tr>
              </thead>
              <tbody>
                {data.accepted.map((r) => (
                  <PeerActionsRow key={r.peer.name} r={r} busy={busy} confirm={confirm} setConfirm={setConfirm} act={act} />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {data.revoked.length > 0 && (
        <div class="section">
          <div class="section-head">
            <h2>Revoked</h2>
            <Badge>{data.revoked.length}</Badge>
          </div>
          <div class="table-wrap">
            <table class="data">
              <thead>
                <tr>
                  <th>Name</th>
                  <th class="hide-sm">URL</th>
                  <th style="text-align:right">Actions</th>
                </tr>
              </thead>
              <tbody>
                {data.revoked.map((p) => (
                  <tr key={p.name}>
                    <td>
                      <a href={`/peers/${encodeURIComponent(p.name)}`} class="mono">
                        {p.name}
                      </a>
                    </td>
                    <td class="mono cell-ellip hide-sm" title={p.url ?? ""}>
                      {p.url ?? "—"}
                    </td>
                    <td style="text-align:right;white-space:nowrap">
                      {confirm?.name === p.name && confirm.action === "delete" ? (
                        <>
                          <span style="font-size:0.8rem;color:var(--bad);margin-right:8px">Delete permanently?</span>
                          <Btn variant="danger" size="sm" disabled={busy !== null} onClick={() => act(p.name, "delete")}>
                            Confirm delete
                          </Btn>{" "}
                          <Btn variant="ghost" size="sm" onClick={() => setConfirm(null)}>
                            Keep
                          </Btn>
                        </>
                      ) : (
                        <>
                          <Btn size="sm" disabled={busy !== null} onClick={() => act(p.name, "accept")}>
                            Re-accept
                          </Btn>{" "}
                          <Btn variant="danger" size="sm" disabled={busy !== null} onClick={() => setConfirm({ name: p.name, action: "delete" })}>
                            Delete
                          </Btn>
                        </>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}

function PeerActionsRow(props: {
  r: PeerRow;
  busy: string | null;
  confirm: { name: string; action: "revoke" | "delete" } | null;
  setConfirm: (c: { name: string; action: "revoke" | "delete" } | null) => void;
  act: (name: string, action: "accept" | "reject" | "revoke" | "delete") => void;
}) {
  const { r } = props;
  const p = r.peer;
  const human = p.kind === "human";
  const health = p.healthy === true ? "ok" : p.healthy === false ? "bad" : "";
  const confirming = props.confirm?.name === p.name && props.confirm?.action === "revoke";
  return (
    <tr>
      <td>
        <span style="display:inline-flex;align-items:center;gap:8px">
          <span class={`avatar`} style={`background:var(--c${identityIdx(p.name)}-bg);color:var(--c${identityIdx(p.name)});width:24px;height:24px;font-size:0.72rem`}>
            {p.name.slice(0, 2).toUpperCase()}
          </span>
          <a href={`/peers/${encodeURIComponent(p.name)}`} class="mono">
            {p.name}
          </a>
          {human && <Badge tone="accent">operator</Badge>}
          {p.channel && <Badge mono title="reverse channel active">⛓</Badge>}
        </span>
      </td>
      <td class="mono cell-ellip hide-sm" title={p.url ?? ""}>
        {p.url && !p.url.startsWith("local://") ? p.url : "—"}
      </td>
      <td class="hide-sm">
        <span style="display:inline-flex;align-items:center;gap:7px">
          <Dot tone={health} class-x="" />
          <span class={`badge ${health === "ok" ? "ok" : health === "bad" ? "bad" : ""}`}>{p.healthy === true ? "healthy" : p.healthy === false ? "unreachable" : "unknown"}</span>
        </span>
      </td>
      <td class="hide-sm" style="white-space:nowrap">
        {r.reqs_1h > 0 ? (
          <span style="display:inline-flex;align-items:center;gap:8px">
            <span class="mono" style="font-size:0.8rem">
              {r.reqs_1h}
            </span>
            <MiniBars series={r.series} />
          </span>
        ) : (
          <span style="color:var(--faint)">—</span>
        )}
      </td>
      <td class="mono hide-sm">
        <RelTimeCell ts={r.last_activity} />
      </td>
      <td style="text-align:right;white-space:nowrap">
        <a href={`/chat?conv=${encodeURIComponent(convWith(p.name))}`}>
          <Btn variant="ghost" size="sm">
            Chat
          </Btn>
        </a>{" "}
        <a href={`/peers/${encodeURIComponent(p.name)}`}>
          <Btn variant="ghost" size="sm">
            Details
          </Btn>
        </a>{" "}
        {confirming ? (
          <>
            <span style="font-size:0.8rem;color:var(--bad);margin-right:6px">Revoke access?</span>
            <Btn variant="danger" size="sm" disabled={props.busy !== null} onClick={() => props.act(p.name, "revoke")}>
              Confirm
            </Btn>{" "}
            <Btn variant="ghost" size="sm" onClick={() => props.setConfirm(null)}>
              Keep
            </Btn>
          </>
        ) : (
          !human && (
            <Btn variant="danger" size="sm" disabled={props.busy !== null} onClick={() => props.setConfirm({ name: p.name, action: "revoke" })}>
              Revoke
            </Btn>
          )
        )}
      </td>
    </tr>
  );
}

function RelTimeCell(props: { ts: number | null }) {
  return <span class="mono" style="font-size:0.8rem">{relTime(props.ts)}</span>;
}

function convWith(name: string): string {
  return `dm:${name}`;
}
