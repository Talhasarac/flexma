import { useEffect, useState } from 'react';
import type { EditorStore } from './store';
import type { PerfSnapshot } from './perf';

const GRAPH_W = 180;
const GRAPH_H = 36;
/** The graph's top: frames this slow or slower fill it. */
const GRAPH_MAX_MS = 50;

const barColor = (ms: number) => (ms <= 17.5 ? '#3ecf6e' : ms <= 34 ? '#f5b83d' : '#f0544f');

/** Frames per second and where frame time goes, over the canvas (⇧⌘.). */
export function PerfOverlay({ store }: { store: EditorStore }) {
  const [s, setS] = useState<PerfSnapshot | null>(null);
  useEffect(() => {
    const tick = () => setS(store.perf.snapshot());
    tick();
    const id = window.setInterval(tick, 250);
    return () => window.clearInterval(id);
  }, [store]);
  if (!s) return null;
  const idle = s.fps === 0;
  const bar = GRAPH_W / 90;
  const y60 = GRAPH_H - (16.7 / GRAPH_MAX_MS) * GRAPH_H;
  return (
    <div className="perf-overlay" aria-hidden="true">
      <div className="perf-fps">
        {idle ? 'idle' : `${s.fps} fps`}
        {!idle && s.frameMs > 0 && <span> · {s.frameMs.toFixed(1)} ms</span>}
      </div>
      <svg width={GRAPH_W} height={GRAPH_H} className="perf-graph">
        <line x1={0} x2={GRAPH_W} y1={y60} y2={y60} className="perf-60" />
        {s.recent.map((ms, i) => {
          const h = Math.max(1, (Math.min(ms, GRAPH_MAX_MS) / GRAPH_MAX_MS) * GRAPH_H);
          return <rect key={i} x={GRAPH_W - (s.recent.length - i) * bar} y={GRAPH_H - h} width={Math.max(1, bar - 0.5)} height={h} fill={barColor(ms)} />;
        })}
      </svg>
      <dl>
        <dt>frame</dt>
        <dd>{idle ? '–' : `${s.frameMs.toFixed(1)} ms, worst ${s.worstFrameMs.toFixed(0)}`}</dd>
        <dt>engine</dt>
        <dd>{idle ? '–' : `${s.engineMs.toFixed(1)} ms, worst ${s.worstEngineMs.toFixed(0)}`}</dd>
        <dt>draw calls</dt>
        <dd>{s.drawCalls.toLocaleString()}</dd>
        <dt>rebuilds</dt>
        <dd>{s.rebuildsPerSec.toFixed(1)}/s</dd>
        <dt>long tasks</dt>
        <dd>{s.longTasks ? `${s.longTasks} in 5 s (${Math.round(s.longTaskMs)} ms)` : 'none'}</dd>
        <dt>memory</dt>
        <dd>
          text {s.textMB.toFixed(0)} MB, images {s.imageMB.toFixed(0)} MB
        </dd>
        <dt>layers</dt>
        <dd>{s.layers.toLocaleString()}</dd>
        <dt>renderer</dt>
        <dd>
          {s.renderer} at {Math.round(s.zoom * 100)}%
        </dd>
      </dl>
    </div>
  );
}
