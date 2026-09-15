// Routing topology: gateway hub, peers on an ellipse, live packet animation.
// v2: wheel zoom + drag pan, full node names, pending peers with inline
// Accept/Reject. Replaces assets/topology.js (0.7.x).

import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { onSse } from "../lib/sse";
import { apiPost } from "../lib/api";
import { toast } from "./ui";
import type { TopoPeer } from "../lib/types";

interface NodePos {
  name: string;
  x: number;
  y: number;
  peer?: TopoPeer;
}

const W = 980;
const H = 420;
const CX = W / 2;
const CY = H / 2;
const RX = 330;
const RY = 150;

interface Flow {
  from: string;
  to: string;
  err: boolean;
  id: number;
}

let flowSeq = 0;

export function Topology(props: { peers: TopoPeer[]; onAccept?: (name: string) => void; onReject?: (name: string) => void }) {
  const [view, setView] = useState({ k: 1, x: 0, y: 0 });
  const [flows, setFlows] = useState<Flow[]>([]);
  const [errEdge, setErrEdge] = useState<Record<string, number>>({}); // dst -> ts
  const dragRef = useRef<{ x: number; y: number; vx: number; vy: number } | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const liveRef = useRef<Set<string>>(new Set());
  const [, force] = useState(0);

  const peers = props.peers;
  const nodes = useMemo<NodePos[]>(() => {
    const visible = peers.filter((p) => p.state !== "revoked");
    const n = visible.length;
    return visible.map((p, i) => {
      // half-step start angle so small fleets sit left/right, not stacked
      const a = (2 * Math.PI * i) / Math.max(n, 1) - Math.PI / 2 + (n > 1 ? Math.PI / n : 0);
      return { name: p.name, x: CX + RX * Math.cos(a), y: CY + RY * Math.sin(a), peer: p };
    });
  }, [peers]);
  const byName = useMemo(() => new Map(nodes.map((n) => [n.name, n])), [nodes]);

  // SSE: animate packets + error edges
  useEffect(() => {
    return onSse("route", (ev) => {
      const e = ev.entry;
      const from = byName.get(e.src) ? e.src : "gateway";
      const to = byName.get(e.dst) ? e.dst : "gateway";
      if (from === to) return;
      const id = ++flowSeq;
      setFlows((f) => [...f.slice(-24), { from, to, err: e.status >= 400, id }]);
      setTimeout(() => setFlows((f) => f.filter((x) => x.id !== id)), 2600);
      const key = e.status >= 400 ? `${e.src}->${e.dst}` : "";
      if (key) {
        setErrEdge((m) => ({ ...m, [key]: Date.now() }));
        setTimeout(() => setErrEdge((m) => {
          const { [key]: _, ...rest } = m;
          return rest;
        }), 8000);
      }
      // live edge brightening
      for (const nm of [e.src, e.dst]) {
        if (nm !== "gateway" && byName.has(nm)) liveRef.current.add(nm);
      }
      force((x) => x + 1);
      setTimeout(() => {
        for (const nm of [e.src, e.dst]) liveRef.current.delete(nm);
        force((x) => x + 1);
      }, 8000);
    });
  }, [byName]);

  // zoom
  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    setView((v) => {
      const k = Math.min(4, Math.max(0.6, v.k * factor));
      return { ...v, k };
    });
  };

  // pan
  const onDown = (e: PointerEvent) => {
    dragRef.current = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y };
    (e.target as Element).setPointerCapture?.(e.pointerId);
  };
  const onMove = (e: PointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    const rect = svgRef.current!.getBoundingClientRect();
    const scale = rect.width / W;
    setView((v) => ({ ...v, x: d.vx + (e.clientX - d.x) / scale / v.k, y: d.vy + (e.clientY - d.y) / scale / v.k }));
  };
  const onUp = () => (dragRef.current = null);

  const accept = async (name: string) => {
    try {
      await apiPost(`/api/peers/${encodeURIComponent(name)}/accept`);
      toast(`Accepted ${name}`, "ok");
      props.onAccept?.(name);
    } catch (e) {
      toast(e instanceof Error ? e.message : "accept failed", "bad");
    }
  };
  const reject = async (name: string) => {
    try {
      await apiPost(`/api/peers/${encodeURIComponent(name)}/reject`);
      toast(`Rejected ${name}`);
      props.onReject?.(name);
    } catch (e) {
      toast(e instanceof Error ? e.message : "reject failed", "bad");
    }
  };

  return (
    <div class="topo-panel">
      <svg
        ref={svgRef}
        viewBox={`0 0 ${W} ${H}`}
        onWheel={onWheel}
        onPointerDown={onDown}
        onPointerMove={onMove}
        onPointerUp={onUp}
        onPointerLeave={onUp}
      >
        <g transform={`translate(${view.x} ${view.y}) scale(${view.k})`}>
          {/* edges */}
          {nodes.map((n) => {
            const errTs = errEdge[`gateway->${n.name}`] ?? errEdge[`${n.name}->gateway`];
            const cls = n.peer?.state === "pending" ? "edge pending" : errTs ? "edge err" : liveRef.current.has(n.name) ? "edge live" : "edge";
            return <path key={n.name} class={cls} d={`M${CX},${CY} L${n.x},${n.y}`} stroke-width={liveRef.current.has(n.name) ? 2 : 1.2} />;
          })}

          {/* gateway node */}
          <g class="topo-node" onClick={() => (location.hash = "")} title="gateway (this switchboard)">
            <rect class="node-box" x={CX - 44} y={CY - 16} width={88} height={32} rx={9} stroke-width="1.4" style="stroke:var(--accent)" />
            <circle cx={CX - 28} cy={CY} r={3.5} fill="var(--ok)" />
            <text class="node-label" x={CX - 18} y={CY + 4}>
              gateway
            </text>
          </g>

          {/* peer nodes */}
          {nodes.map((n) => {
            const p = n.peer!;
            const pending = p.state === "pending";
            const health = pending ? "warn" : p.healthy === true ? "ok" : p.healthy === false ? "bad" : "";
            const count = p.reqs_1h > 0 ? `${p.reqs_1h}↗` : "";
            // size the pill for the FULL name; ellipsize only beyond 250px
            const icons = (p.channel ? 20 : 0) + (count ? 24 : 0);
            const w = Math.min(280, Math.max(96, n.name.length * 8.6 + 30 + icons));
            return (
              <g
                key={n.name}
                class="topo-node"
                onClick={(e) => {
                  if ((e.target as Element).closest("button")) return;
                  if (!pending) location.href = `/peers/${encodeURIComponent(n.name)}`;
                }}
              >
                <rect class="node-box" x={n.x - w / 2} y={n.y - 15} width={w} height={30} rx={15} stroke-width={pending ? 1.6 : 1.2} style={pending ? "stroke:var(--warn)" : undefined} />
                <circle cx={n.x - w / 2 + 13} cy={n.y} r={3.5} fill={health === "ok" ? "var(--ok)" : health === "bad" ? "var(--bad)" : health === "warn" ? "var(--warn)" : "var(--faint)"}>
                  {health === "bad" && <animate attributeName="opacity" values="1;0.3;1" dur="1.6s" repeatCount="indefinite" />}
                </circle>
                <text class="node-label" x={n.x - w / 2 + 22} y={n.y + 1} style={pending ? "fill:var(--warn)" : undefined}>
                  {truncName(n.name, (w - 30 - icons) / 8.2)}
                </text>
                {p.channel && (
                  <text class="node-sub" x={n.x + w / 2 - 8} y={n.y + 1} text-anchor="end">
                    ⛓
                  </text>
                )}
                {count && (
                  <text class="node-sub" x={n.x + w / 2 - (p.channel ? 22 : 8)} y={n.y + 1} text-anchor="end">
                    {count}
                  </text>
                )}
                {pending && (
                  <g>
                    <g transform={`translate(${n.x - 42},${n.y + 22})`} onClick={(e) => { e.stopPropagation(); accept(n.name); }}>
                      <rect width={40} height={18} rx={4} fill="var(--ok-bg)" style="cursor:pointer" />
                      <text x={20} y={12.5} text-anchor="middle" font-size="10" fill="var(--ok)" style="cursor:pointer" font-weight="600">
                        Accept
                      </text>
                    </g>
                    <g transform={`translate(${n.x + 2},${n.y + 22})`} onClick={(e) => { e.stopPropagation(); reject(n.name); }}>
                      <rect width={40} height={18} rx={4} fill="var(--bad-bg)" style="cursor:pointer" />
                      <text x={20} y={12.5} text-anchor="middle" font-size="10" fill="var(--bad)" style="cursor:pointer" font-weight="600">
                        Reject
                      </text>
                    </g>
                  </g>
                )}
              </g>
            );
          })}

          {/* animated packets */}
          {flows.map((f) => {
            const a = f.from === "gateway" ? { x: CX, y: CY } : byName.get(f.from);
            const b = f.to === "gateway" ? { x: CX, y: CY } : byName.get(f.to);
            if (!a || !b) return null;
            const id = `pk${f.id}`;
            return (
              <g key={f.id}>
                <circle r={3.2} fill={f.err ? "var(--bad)" : "var(--accent)"}>
                  <animateMotion id={id} dur="1.2s" fill="freeze" {...({ path: `M${a.x},${a.y} L${b.x},${b.y}` } as Record<string, string>)} />
                </circle>
              </g>
            );
          })}
        </g>
      </svg>
      <div class="topo-legend">
        <span style="display:inline-flex;align-items:center;gap:5px">
          <span class="dot ok" /> healthy
        </span>
        <span style="display:inline-flex;align-items:center;gap:5px">
          <span class="dot warn" /> pending
        </span>
        <span style="display:inline-flex;align-items:center;gap:5px">
          <span class="dot bad" /> unreachable
        </span>
        <span style="display:inline-flex;align-items:center;gap:5px">
          <span class="dot" /> unknown
        </span>
        <span>⛓ reverse channel</span>
        <span style="margin-left:auto;color:var(--faint)">scroll to zoom · drag to pan · click a peer for detail</span>
      </div>
    </div>
  );
}

function truncName(name: string, maxChars: number): string {
  const max = Math.max(6, Math.floor(maxChars));
  return name.length > max ? name.slice(0, max - 1) + "…" : name;
}
