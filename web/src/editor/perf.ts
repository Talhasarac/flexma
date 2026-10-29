// Frame timing for the performance overlay (⇧⌘. or the main menu, `?perf`
// in the URL), and for debugging from the console: `__flexma.perf.snapshot()`.
import { useSyncExternalStore } from 'react';
import { gpuDevice, type FlexmaEditor } from '../engine';
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
  /** From a frame's submission until the GPU finished it (ms), median; -1
   * when unknown (WebGL2). Close to `frameMs`: the GPU is the slow part. */
  gpuMs: number;
  /** Texts drawn into textures per second (zooming redraws visible text). */
  textRastersPerSec: number;
  /** Frames the GPU rejected (and redrew), in all. */
  gpuErrors: number;
  /** Rebuilds per second by cause. */
  rebuildsFrom: { gpuRetry: number; invalidated: number; document: number };
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
  private prev: { t: number; rebuilds: number; rasters: number; by: number[] } | null = null;
  private gpu: { t: number; ms: number }[] = [];
  private gpuPending = false;

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
    // One measurement in flight at a time: when the GPU finishes this frame.
    if (gpuDevice && !this.gpuPending) {
      this.gpuPending = true;
      const sent = performance.now();
      void gpuDevice.queue.onSubmittedWorkDone().then(() => {
        this.gpuPending = false;
        this.gpu.push({ t: sent, ms: performance.now() - sent });
        if (this.gpu.length > RECENT) this.gpu.shift();
      });
    }
    while (this.frames.length && this.frames[0].t < t - 1000) this.frames.shift();
  }

  snapshot(): PerfSnapshot {
    const now = performance.now();
    const frames = this.frames.filter((f) => f.t >= now - 1000);
    const gaps = this.gaps.filter((g) => g.t >= now - 1000).map((g) => g.ms);
    this.long = this.long.filter((l) => l.t >= now - 5000);
    const st = JSON.parse(this.engine.stats()) as Record<string, unknown>;
    const n = Number(st.rebuilds ?? 0);
    const rasters = Number(st.textRasters ?? 0);
    const byObj = (st.rebuildsBy ?? {}) as Record<string, number>;
    const by = [byObj.gpuRetry ?? 0, byObj.invalidated ?? 0, byObj.document ?? 0];
    const prev = this.prev;
    this.prev = { t: now, rebuilds: n, rasters, by };
    const perSec = (a: number, b: number) => (prev && now > prev.t ? ((a - b) * 1000) / (now - prev.t) : 0);
    const engineMs = frames.map((f) => f.ms);
    const gpu = this.gpu.filter((g) => g.t >= now - 1000).map((g) => g.ms);
    return {
      fps: frames.length,
      frameMs: median(gaps),
      worstFrameMs: gaps.length ? Math.max(...gaps) : 0,
      engineMs: median(engineMs),
      worstEngineMs: engineMs.length ? Math.max(...engineMs) : 0,
      longTasks: this.long.length,
      longTaskMs: this.long.reduce((a, l) => a + l.ms, 0),
      drawCalls: Number(st.drawCalls ?? 0),
      rebuildsPerSec: perSec(n, prev?.rebuilds ?? n),
      gpuMs: gpuDevice ? median(gpu) : -1,
      textRastersPerSec: perSec(rasters, prev?.rasters ?? rasters),
      gpuErrors: Number(st.gpuErrors ?? 0),
      rebuildsFrom: { gpuRetry: perSec(by[0], prev?.by[0] ?? by[0]), invalidated: perSec(by[1], prev?.by[1] ?? by[1]), document: perSec(by[2], prev?.by[2] ?? by[2]) },
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
