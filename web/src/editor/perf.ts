// Frame timing for the performance overlay (⇧⌘. or the main menu, `?perf`
// in the URL), and for debugging from the console: `__flexma.perf.snapshot()`.
import { useSyncExternalStore } from 'react';
import type { FlexmaEditor } from '../engine';
import { readStorage, writeStorage } from '../lib/storage';

export interface PerfSnapshot {
  /** Frames drawn in the last second (0 while nothing changes on screen). */
  fps: number;
  /** Time between frames while drawing continuously (ms): median and worst
   * in the last second. Far above `engineMs`: the GPU or the browser is the
   * slow part. */
  frameMs: number;
  worstFrameMs: number;
  /** The engine's own work per drawn frame (ms): median and worst. */
  engineMs: number;
  worstEngineMs: number;
  /** Main-thread tasks over 50 ms in the last 5 s, and their total (ms). */
  longTasks: number;
  longTaskMs: number;
  /** From the engine's last frame. */
  drawCalls: number;
  rebuildsPerSec: number;
  textMB: number;
  imageMB: number;
  layers: number;
  renderer: string;
  zoom: number;
  /** The last 90 frame intervals (ms), oldest first, for the graph. */
  recent: number[];
}

/** Frames this far apart weren't slow: nothing needed drawing in between. */
const IDLE_GAP_MS = 250;
const RECENT = 90;

const median = (xs: number[]) => {
  if (!xs.length) return 0;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)];
};

export class PerfMonitor {
  private engine: FlexmaEditor;
  /** Drawn frames: when, and the engine's time on them. */
  private frames: { t: number; ms: number }[] = [];
  /** Gaps between consecutive drawn frames, while drawing continuously. */
  private gaps: { t: number; ms: number }[] = [];
  private long: { t: number; ms: number }[] = [];
  private lastDrawn = 0;
  private rebuilds: { t: number; n: number } | null = null;

  constructor(engine: FlexmaEditor) {
    this.engine = engine;
    try {
      new PerformanceObserver((list) => {
        for (const e of list.getEntries()) this.long.push({ t: e.startTime, ms: e.duration });
      }).observe({ type: 'longtask' });
    } catch {
      // Not every browser reports long tasks.
    }
  }

  /** One pass of the render loop: `ms` spent in `engine.frame`, `drew` if it drew. */
  record(t: number, ms: number, drew: boolean) {
    if (!drew) return;
    if (this.lastDrawn && t - this.lastDrawn < IDLE_GAP_MS) {
      this.gaps.push({ t, ms: t - this.lastDrawn });
      if (this.gaps.length > RECENT * 2) this.gaps.splice(0, this.gaps.length - RECENT);
    }
    this.lastDrawn = t;
    this.frames.push({ t, ms });
    while (this.frames.length && this.frames[0].t < t - 1000) this.frames.shift();
  }

  snapshot(): PerfSnapshot {
    const now = performance.now();
    const frames = this.frames.filter((f) => f.t >= now - 1000);
    const gaps = this.gaps.filter((g) => g.t >= now - 1000).map((g) => g.ms);
    this.long = this.long.filter((l) => l.t >= now - 5000);
    const st = JSON.parse(this.engine.stats()) as Record<string, number | string>;
    const n = Number(st.rebuilds ?? 0);
    const prev = this.rebuilds;
    this.rebuilds = { t: now, n };
    const engineMs = frames.map((f) => f.ms);
    return {
      fps: frames.length,
      frameMs: median(gaps),
      worstFrameMs: gaps.length ? Math.max(...gaps) : 0,
      engineMs: median(engineMs),
      worstEngineMs: engineMs.length ? Math.max(...engineMs) : 0,
      longTasks: this.long.length,
      longTaskMs: this.long.reduce((a, l) => a + l.ms, 0),
      drawCalls: Number(st.drawCalls ?? 0),
      rebuildsPerSec: prev && now > prev.t ? ((n - prev.n) * 1000) / (now - prev.t) : 0,
      textMB: Number(st.textMemoryMB ?? 0),
      imageMB: Number(st.imageMemoryMB ?? 0),
      layers: Number(st.nodes ?? 0),
      renderer: String(st.renderer ?? ''),
      zoom: Number(st.zoom ?? 1),
      recent: this.gaps.slice(-RECENT).map((g) => g.ms),
    };
  }
}

// ---- whether the overlay shows ----

const KEY = 'flexma:perfOverlay';
let shown = readStorage(KEY) === '1' || new URLSearchParams(window.location.search).has('perf');
const listeners = new Set<() => void>();

export function togglePerfOverlay() {
  shown = !shown;
  writeStorage(KEY, shown ? '1' : null);
  for (const l of listeners) l();
}

export function usePerfOverlay(): boolean {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => shown,
  );
}
