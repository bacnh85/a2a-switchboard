// Hand-rolled SVG charts: sparklines (KPI tiles) and the dashboard area chart
// with hover crosshair + tooltip. No chart library — keeps the bundle tiny.

import { useState, useRef } from "preact/hooks";
import { fmtMs } from "../lib/format";
import type { Bucket } from "../lib/types";

function path(values: number[], w: number, h: number, max: number): string {
  if (values.length === 0) return "";
  const step = values.length > 1 ? w / (values.length - 1) : w;
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * h).toFixed(1)}`);
  return `M${pts.join(" L")}`;
}

/** Tiny area+line sparkline for KPI tiles. */
export function Sparkline(props: { values: number[]; tone?: "accent" | "bad" }) {
  const w = 160;
  const h = 26;
  const v = props.values;
  const max = Math.max(1, ...v);
  const line = path(v, w, h, max);
  const area = v.length > 0 ? `${line} L${w},${h} L0,${h} Z` : "";
  const color = props.tone === "bad" ? "var(--chart-err)" : "var(--chart-1)";
  const gid = `sg-${props.tone ?? "a"}`;
  return (
    <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden="true">
      <defs>
        <linearGradient id={gid} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color={color} stop-opacity="0.28" />
          <stop offset="100%" stop-color={color} stop-opacity="0.02" />
        </linearGradient>
      </defs>
      {area && <path d={area} fill={`url(#${gid})`} />}
      {line && <path d={line} fill="none" stroke={color} stroke-width="1.5" />}
    </svg>
  );
}

/** Per-peer 1h activity: a row of minute bars. */
export function MiniBars(props: { series: number[] }) {
  const w = 90;
  const h = 18;
  const v = props.series;
  const n = v.length || 1;
  const max = Math.max(1, ...v);
  const bw = Math.max(1.5, w / n - 0.6);
  return (
    <svg viewBox={`0 0 ${w} ${h}`} width={w} height={h} aria-hidden="true">
      {v.map((x, i) =>
        x > 0 ? (
          <rect key={i} x={((w / n) * i).toFixed(1)} y={(h - (x / max) * h).toFixed(1)} width={bw} height={((x / max) * h).toFixed(1)} rx="1" fill="var(--chart-1)" opacity="0.8" />
        ) : null,
      )}
    </svg>
  );
}

export interface AreaSeries {
  name: string;
  color: "accent" | "bad";
  values: number[];
}

/** Dashboard traffic chart: stacked bars (routed vs errors) + p95 line. */
export function TrafficChart(props: { buckets: Bucket[]; windowSec: number }) {
  const [hover, setHover] = useState<number | null>(null);
  const [tip, setTip] = useState<{ x: number; y: number } | null>(null);
  const ref = useRef<SVGSVGElement>(null);
  const W = 1000;
  const H = 220;
  const padL = 38;
  const padB = 22;
  const padT = 8;
  const innerW = W - padL - 6;
  const innerH = H - padB - padT;
  const bs = props.buckets;
  const n = bs.length || 1;
  const maxRouted = Math.max(1, ...bs.map((b) => b.routed));
  const maxP95 = Math.max(...bs.map((b) => b.p95 ?? 0), 1);
  const slot = innerW / n;
  const bw = Math.max(1.5, Math.min(18, slot - Math.min(2, slot * 0.2)));

  const y = (v: number) => padT + innerH - (v / maxRouted) * innerH;
  const p95y = (v: number) => padT + innerH - (v / maxP95) * innerH;

  const gridLines = [0, 0.25, 0.5, 0.75, 1].map((f) => ({
    y: padT + innerH - f * innerH,
    v: Math.round(maxRouted * f),
  }));

  const onMove = (e: PointerEvent) => {
    if (!ref.current) return;
    const rect = ref.current.getBoundingClientRect();
    const px = ((e.clientX - rect.left) / rect.width) * W;
    const i = Math.floor((px - padL) / slot);
    if (i >= 0 && i < n) {
      setHover(i);
      setTip({ x: e.clientX, y: e.clientY });
    } else {
      setHover(null);
    }
  };

  const p95path = bs
    .map((b, i) => `${i === 0 ? "M" : "L"}${(padL + slot * i + slot / 2).toFixed(1)},${p95y(b.p95 ?? 0).toFixed(1)}`)
    .join(" ");

  const hb = hover !== null ? bs[hover] : null;

  return (
    <div style="position:relative">
      <svg
        ref={ref}
        viewBox={`0 0 ${W} ${H}`}
        style="display:block;width:100%;height:230px"
        onPointerMove={onMove}
        onPointerLeave={() => {
          setHover(null);
          setTip(null);
        }}
        role="img"
        aria-label="Traffic over time"
      >
        {gridLines.map((g, i) => (
          <g key={i}>
            <line x1={padL} x2={W - 6} y1={g.y} y2={g.y} stroke="var(--chart-grid)" stroke-width="1" />
            <text x={padL - 6} y={g.y + 3} text-anchor="end" font-size="9" fill="var(--faint)" font-family="ui-monospace,monospace">
              {g.v}
            </text>
          </g>
        ))}
        {bs.map((b, i) => {
          const x = padL + slot * i + (slot - bw) / 2;
          const yErr = y(b.errors);
          const errH = (b.errors / maxRouted) * innerH;
          const okH = ((b.routed - b.errors) / maxRouted) * innerH;
          return (
            <g key={i}>
              {b.routed > 0 && (
                <>
                  <rect x={x} y={yErr - okH} width={bw} height={okH} rx="1.5" fill="var(--chart-1)" opacity={hover === i ? 1 : 0.75} />
                  {b.errors > 0 && <rect x={x} y={yErr} width={bw} height={Math.max(1.5, errH)} rx="1.5" fill="var(--chart-err)" />}
                </>
              )}
            </g>
          );
        })}
        <path d={p95path} fill="none" stroke="var(--warn)" stroke-width="1.4" stroke-dasharray="3 3" opacity="0.9" />
        {hover !== null && (
          <line x1={padL + slot * hover + slot / 2} x2={padL + slot * hover + slot / 2} y1={padT} y2={padT + innerH} stroke="var(--border-strong)" stroke-width="1" />
        )}
        {/* x-axis time labels: first / middle / last bucket */}
        {bs.length > 1 &&
          [0, Math.floor((n - 1) / 2), n - 1].map((i, k) => {
            const d = new Date(bs[i].ts * 1000);
            const p = (x: number) => String(x).padStart(2, "0");
            const lbl = `${p(d.getHours())}:${p(d.getMinutes())}`;
            return (
              <text key={k} x={padL + slot * i + slot / 2} y={H - 6} text-anchor={k === 0 ? "start" : k === 2 ? "end" : "middle"} font-size="9" fill="var(--faint)" font-family="ui-monospace,monospace">
                {lbl}
              </text>
            );
          })}
      </svg>
      {hb && tip && (
        <div class="chart-tip" style={`left:${tip.x + 14}px;top:${tip.y + 12}px`}>
          <div style="color:var(--faint);margin-bottom:3px">{new Date(hb.ts * 1000).toLocaleTimeString()}</div>
          <div class="tip-row">
            <span>
              <span class="sw" style="background:var(--chart-1)" />
              routed
            </span>
            <b>{hb.routed}</b>
          </div>
          <div class="tip-row">
            <span>
              <span class="sw" style="background:var(--chart-err)" />
              errors
            </span>
            <b>{hb.errors}</b>
          </div>
          <div class="tip-row">
            <span>
              <span class="sw" style="background:var(--warn)" />
              p95
            </span>
            <b>{hb.p95 != null ? fmtMs(hb.p95) : "—"}</b>
          </div>
        </div>
      )}
      <div style="display:flex;gap:16px;padding:6px 4px 0;font-size:0.75rem;color:var(--muted)">
        <span>
          <span class="sw" style="display:inline-block;width:8px;height:8px;border-radius:2px;background:var(--chart-1);margin-right:5px" />
          routed
        </span>
        <span>
          <span class="sw" style="display:inline-block;width:8px;height:8px;border-radius:2px;background:var(--chart-err);margin-right:5px" />
          errors
        </span>
        <span>
          <span class="sw" style="display:inline-block;width:8px;height:2px;border-top:2px dashed var(--warn);margin-right:5px;vertical-align:3px" />
          p95 latency
        </span>
      </div>
    </div>
  );
}
