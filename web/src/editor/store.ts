// EditorStore: the bridge between the wasm engine, the multiplayer socket and
// React. React subscribes with useSyncExternalStore; snapshots are cached per
// engine version counter so components re-render only when their slice changes.
import { useSyncExternalStore } from 'react';
import type { FlexmaEditor } from '../engine';
import { api, exportPngUrl, exportSvgUrl, shareUrls, type CommentThread, type ExportOptions, type PngExportOptions } from '../lib/api';
import type { BrushSettings, ComponentInfo, ConnectionStatus, EditingText, LayerNode, NodeData, PageInfo, Peer, StyleInfo, Tool } from './types';
import { FontManager } from './fonts';

type Listener = () => void;

const SEND_INTERVAL_MS = 33; // ~30 batches per second (Figma's cadence)
/** Brush and eraser settings are remembered per browser. */
const BRUSH_KEY = (eraser: boolean) => (eraser ? 'flexma:eraser' : 'flexma:brush');
/** The pen's pressure curve, per browser (it's about the user's hand and tablet). */
const CURVE_KEY = 'flexma:pressureCurve';
const PRESENCE_INTERVAL_MS = 50;

interface UiState {
  tool: Tool;
  zoom: number;
  canUndo: boolean;
  canRedo: boolean;
  canEdit: boolean;
  page: string;
  editingText: EditingText | null;
}

export interface ServerMessage {
  t: string;
  [key: string]: unknown;
}

export class EditorStore {
  readonly engine: FlexmaEditor;
  readonly fileId: string;
  /** Set when the file was opened through a public view-only link. */
  readonly shareToken: string | null;
  /** The file's workspace (members only), for its fonts. */
  readonly workspaceId: string | null;
  /** The signed-in user, whose other tabs are hidden from this one. */
  readonly userId: string | null;
  /** A painting file: the brush and eraser are shown. */
  paintTools: boolean;
  /** Pen pressure curve, −1 (light touch) … 1 (firm). */
  pressureCurve = 0;
  readonly fonts: FontManager;
  private listeners = new Set<Listener>();
  private socket: WebSocket | null = null;
  private retry = 0;
  private closed = false;
  private lastSend = 0;
  private lastPresence = 0;
  private presenceDirty = false;
  private cursorWorld: [number, number] | null = null;
  private loadedOnce = false;
  onFatal: ((message: string) => void) | null = null;
  onReject: ((message: string) => void) | null = null;
  onReload: (() => void) | null = null;
  onLoaded: (() => void) | null = null;

  // cached snapshots
  private v = { structure: -1, selection: -1, ui: -1, zoom: -1, pages: -1 };
  private layers: LayerNode[] = [];
  private components: ComponentInfo[] = [];
  private styles: StyleInfo[] = [];
  private selection: NodeData[] = [];
  private pages: { pages: PageInfo[]; current: string } = { pages: [], current: '' };
  private ui: UiState = {
    tool: 'select',
    zoom: 1,
    canUndo: false,
    canRedo: false,
    canEdit: true,
    page: '',
    editingText: null,
  };
  /** Every other connection in the file, one per tab. */
  private allPeers: Peer[] = [];
  /** When each connection last moved its cursor, selected or changed page. */
  private activeAt = new Map<string, number>();
  /** The peers shown: each person's last active tab, none of this user's other tabs. */
  peers: Peer[] = [];
  comments: CommentThread[] = [];
  commentMode = false;
  /** Thread whose popover is open ('draft' for a new comment being written). */
  openThread: string | null = null;
  status: ConnectionStatus = 'connecting';
  clientId = '';
  role: string = 'viewer';
  saving = false;
  private notifyQueued = false;
  private flushTimer = 0;
  /** Bumped on every (re)load, so a stale paint-state fetch is dropped. */
  private paintGen = 0;

  constructor(
    engine: FlexmaEditor,
    fileId: string,
    opts: { shareToken?: string | null; workspaceId?: string | null; userId?: string | null; paintTools?: boolean } = {},
  ) {
    const shareToken = opts.shareToken ?? null;
    this.engine = engine;
    this.fileId = fileId;
    this.shareToken = shareToken;
    this.workspaceId = opts.workspaceId ?? null;
    this.userId = opts.userId ?? null;
    this.paintTools = opts.paintTools ?? false;
    engine.set_paint_tools(this.paintTools);
    try {
      const c = Number(localStorage.getItem(CURVE_KEY));
      if (Number.isFinite(c)) this.pressureCurve = Math.max(-1, Math.min(1, c));
    } catch {
      // Storage may be unavailable; the default curve is fine.
    }
    for (const eraser of [false, true]) {
      try {
        const saved = localStorage.getItem(BRUSH_KEY(eraser));
        if (saved) engine.set_brush(eraser, saved);
      } catch {
        // Storage may be unavailable; the defaults are fine.
      }
    }
    this.fonts = new FontManager(this);
    // Guests have no session cookie, so images load through the link instead.
    if (shareToken) engine.set_asset_base(shareUrls.assetBase(shareToken));
    engine.set_paint_base(shareToken ? api.paint.shareTileBase(shareToken) : api.paint.tileBase(fileId));
  }

  // ---------------- URLs (work for members and link guests) ----------------

  exportPngUrl(opts: PngExportOptions = {}) {
    return this.shareToken ? shareUrls.exportPng(this.shareToken, opts) : exportPngUrl(this.fileId, opts);
  }

  exportSvgUrl(opts: ExportOptions = {}) {
    return this.shareToken ? shareUrls.exportSvg(this.shareToken, opts) : exportSvgUrl(this.fileId, opts);
  }

  assetUrl(assetId: string) {
    return this.shareToken ? shareUrls.asset(this.shareToken, assetId) : api.assets.url(assetId);
  }

  // ---------------- subscription ----------------

  subscribe = (l: Listener) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  /** Re-read engine versions and notify React if anything changed. */
  poll() {
    const e = this.engine;
    let changed = false;
    const sv = e.structure_version();
    if (sv !== this.v.structure) {
      this.v.structure = sv;
      this.layers = JSON.parse(e.layers()) as LayerNode[];
      this.components = JSON.parse(e.components()) as ComponentInfo[];
      this.styles = JSON.parse(e.styles()) as StyleInfo[];
      this.pages = JSON.parse(e.pages()) as { pages: PageInfo[]; current: string };
      this.fonts.schedule();
      changed = true;
    }
    const sel = e.selection_version();
    if (sel !== this.v.selection) {
      this.v.selection = sel;
      this.selection = JSON.parse(e.selection()) as NodeData[];
      this.presenceDirty = true;
      changed = true;
    }
    const uv = e.ui_version();
    const zoom = e.zoom();
    if (uv !== this.v.ui || zoom !== this.v.zoom) {
      this.v.ui = uv;
      this.v.zoom = zoom;
      const editing = e.editing_text();
      const page = e.current_page();
      if (page !== this.ui.page) this.presenceDirty = true;
      this.ui = {
        tool: e.tool() as Tool,
        zoom,
        canUndo: e.can_undo(),
        canRedo: e.can_redo(),
        canEdit: e.can_edit(),
        page,
        editingText: editing === 'null' ? null : (JSON.parse(editing) as EditingText),
      };
      changed = true;
    }
    const saving = e.unacked() > 0 || e.has_outgoing();
    if (saving !== this.saving) {
      this.saving = saving;
      changed = true;
    }
    if (changed) this.notify();
  }

  notify() {
    if (this.notifyQueued) return;
    this.notifyQueued = true;
    queueMicrotask(() => {
      this.notifyQueued = false;
      for (const l of this.listeners) l();
    });
  }

  getLayers = () => this.layers;
  getComponents = () => this.components;
  getStyles = () => this.styles;
  getSelection = () => this.selection;
  getPages = () => this.pages;
  getUi = () => this.ui;
  getPeers = () => this.peers;
  getComments = () => this.comments;
  getCommentMode = () => this.commentMode;
  getOpenThread = () => this.openThread;

  async refreshComments() {
    if (this.shareToken) return; // comments are for workspace members only
    try {
      this.comments = await api.comments.list(this.fileId);
      this.notify();
    } catch {
      /* offline: keep the last list */
    }
  }

  setCommentMode(on: boolean) {
    if (this.commentMode === on || (on && this.shareToken)) return;
    this.commentMode = on;
    if (on) this.engine.set_tool('select');
    else if (this.openThread === 'draft') this.openThread = null;
    this.notify();
  }

  setOpenThread(id: string | null) {
    this.openThread = id;
    this.notify();
  }
  getStatus = () => this.status;
  getPaintTools = () => this.paintTools;
  getSaving = () => this.saving;

  // ---------------- network ----------------

  connect(url: string) {
    this.closed = false;
    // rAF pauses in background tabs; a timer keeps ops and presence flowing.
    if (!this.flushTimer) this.flushTimer = window.setInterval(() => this.flush(), 50);
    this.status = 'connecting';
    this.notify();
    const ws = new WebSocket(url);
    this.socket = ws;
    ws.onmessage = (ev) => this.handleMessage(typeof ev.data === 'string' ? ev.data : '');
    ws.onclose = () => {
      if (this.socket !== ws) return;
      this.socket = null;
      this.status = 'offline';
      this.activeAt.clear();
      this.setPeers([]);
      if (!this.closed) {
        const delay = Math.min(10_000, 500 * 2 ** this.retry++);
        window.setTimeout(() => {
          if (!this.closed) this.connect(url);
        }, delay);
      }
    };
  }

  close() {
    this.closed = true;
    window.clearInterval(this.flushTimer);
    this.flushTimer = 0;
    this.flush(true);
    this.socket?.close();
    this.socket = null;
  }

  private handleMessage(raw: string) {
    let msg: ServerMessage;
    try {
      msg = JSON.parse(raw) as ServerMessage;
    } catch {
      return;
    }
    switch (msg.t) {
      case 'init': {
        this.retry = 0;
        this.clientId = String(msg.clientId);
        this.role = String(msg.role);
        this.engine.load(JSON.stringify(msg.doc), this.clientId, this.role !== 'viewer');
        void this.loadPaint();
        this.activeAt.clear();
        this.setPeers(((msg.peers as Peer[]) ?? []).filter((p) => p.clientId !== this.clientId));
        this.status = 'online';
        this.presenceDirty = true;
        void this.refreshComments();
        void this.fonts.refresh();
        if (!this.loadedOnce) {
          this.loadedOnce = true;
          this.engine.zoom_to_fit_initial();
          this.onLoaded?.();
        }
        this.v.structure = -1;
        this.v.selection = -1;
        this.v.ui = -1;
        this.poll();
        break;
      }
      case 'ops': {
        this.engine.receive_ops(String(msg.from), Number(msg.batch), JSON.stringify(msg.ops));
        break;
      }
      case 'presence': {
        const peer = msg.peer as Peer;
        if (peer.clientId === this.clientId) return;
        const i = this.allPeers.findIndex((p) => p.clientId === peer.clientId);
        if (i >= 0 && isActivity(this.allPeers[i], peer)) this.activeAt.set(peer.clientId, performance.now());
        this.setPeers(i >= 0 ? this.allPeers.map((p, j) => (j === i ? peer : p)) : [...this.allPeers, peer]);
        break;
      }
      case 'strokeLive':
      case 'stroke':
      case 'strokeUndo':
      case 'tile':
        this.engine.receive_paint(raw);
        break;
      case 'paintReject':
        this.engine.receive_paint(raw);
        this.onReject?.(String(msg.error ?? 'Brush stroke rejected'));
        break;
      case 'leave': {
        this.engine.paint_peer_left(String(msg.clientId));
        this.activeAt.delete(String(msg.clientId));
        this.setPeers(this.allPeers.filter((p) => p.clientId !== msg.clientId));
        break;
      }
      case 'comments':
        void this.refreshComments();
        break;
      case 'fonts':
        // Someone added or removed workspace fonts.
        void this.fonts.refresh();
        break;
      case 'reload':
        // The document was replaced (version restore); the socket closes and we
        // reconnect to load it.
        this.onReload?.();
        break;
      case 'reject':
        this.onReject?.(String(msg.error ?? 'Change rejected'));
        break;
      case 'error':
        if (msg.fatal) {
          this.closed = true;
          this.onFatal?.(String(msg.error));
        }
        break;
    }
  }

  /** Show one cursor, selection and avatar per person: the tab they used last. */
  private setPeers(all: Peer[]) {
    this.allPeers = all;
    const rank = (p: Peer) => this.activeAt.get(p.clientId) ?? (p.cursor ? 0 : -1);
    const shown = new Map<string, Peer>();
    for (const p of all) {
      // Agents carry their owner's user id but are separate collaborators.
      if (p.isAgent) {
        shown.set(`agent:${p.clientId}`, p);
        continue;
      }
      if (p.userId === this.userId) continue;
      const cur = shown.get(p.userId);
      if (!cur || rank(p) >= rank(cur)) shown.set(p.userId, p);
    }
    this.peers = [...shown.values()];
    this.engine.set_peers(JSON.stringify(this.peers));
    this.notify();
  }

  /** Send queued ops (throttled) and presence. Called every animation frame. */
  flush(force = false) {
    const now = performance.now();
    const ws = this.socket;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    if (force || now - this.lastSend >= SEND_INTERVAL_MS) {
      const out = this.engine.take_outgoing();
      if (out) {
        ws.send(out);
        this.lastSend = now;
      }
      // After the ops, so a new paint layer exists before its first stroke.
      const paint = this.engine.take_paint_messages();
      if (paint) {
        for (const m of paint.split('\n')) ws.send(m);
        this.lastSend = now;
      }
    }
    this.uploadPaintSnapshots();
    if (this.presenceDirty && (force || now - this.lastPresence >= PRESENCE_INTERVAL_MS)) {
      this.presenceDirty = false;
      this.lastPresence = now;
      ws.send(
        JSON.stringify({
          t: 'presence',
          cursor: this.cursorWorld,
          selection: JSON.parse(this.engine.selection_ids()) as string[],
          page: this.engine.current_page(),
        }),
      );
    }
  }

  // ---------------- paint layers ----------------

  /** Show or hide the brush and eraser (the file became a painting or design file). */
  setPaintTools(on: boolean) {
    this.paintTools = on;
    this.engine.set_paint_tools(on);
    const tool = this.engine.tool();
    if (!on && (tool === 'brush' || tool === 'eraser')) this.engine.set_tool('select');
    this.v.ui = -1;
    this.poll();
    this.notify();
  }

  setPressureCurve(curve: number) {
    this.pressureCurve = Math.max(-1, Math.min(1, curve));
    try {
      localStorage.setItem(CURVE_KEY, String(this.pressureCurve));
    } catch {
      // Not remembered; nothing else depends on it.
    }
    this.notify();
  }

  brush(eraser: boolean): BrushSettings {
    return JSON.parse(this.engine.brush(eraser)) as BrushSettings;
  }

  setBrush(eraser: boolean, patch: Partial<BrushSettings>) {
    this.engine.set_brush(eraser, JSON.stringify(patch));
    try {
      localStorage.setItem(BRUSH_KEY(eraser), this.engine.brush(eraser));
    } catch {
      // Not remembered; nothing else depends on it.
    }
    this.poll();
  }

  /** Fetch the file's paint state (tile snapshots plus the strokes on top). */
  private async loadPaint() {
    const gen = ++this.paintGen;
    for (let attempt = 0; attempt < 5; attempt++) {
      try {
        const state = this.shareToken ? await api.paint.shareState(this.shareToken) : await api.paint.state(this.fileId);
        if (gen === this.paintGen) this.engine.load_paint(JSON.stringify(state));
        return;
      } catch {
        await new Promise((r) => window.setTimeout(r, 500 * 2 ** attempt));
        if (gen !== this.paintGen || this.closed) return;
      }
    }
  }

  /** Upload the tile snapshots the engine read back (only the last painter does). */
  private uploadPaintSnapshots() {
    const snaps = this.engine.take_paint_snapshots();
    for (let i = 0; i + 1 < snaps.length; i += 2) {
      const meta = JSON.parse(snaps[i] as string) as { layer: string; x: number; y: number; kind: 'base' | 'view'; seq: number };
      const pixels = snaps[i + 1] as Uint8Array;
      if (this.shareToken || this.role === 'viewer') continue;
      void tilePng(pixels)
        .then((png) => api.paint.putTile(this.fileId, meta.layer, meta.x, meta.y, meta.kind, meta.seq, png))
        .catch(() => this.engine.paint_snapshot_failed(meta.layer, meta.x, meta.y));
    }
  }

  setCursor(world: [number, number] | null) {
    this.cursorWorld = world;
    this.presenceDirty = true;
  }
}

/**
 * Whether a peer's new presence means someone is using that tab. The first
 * update after joining doesn't count (reconnecting background tabs send one),
 * and neither does the cursor leaving the canvas (switching tabs fires that).
 */
function isActivity(prev: Peer, next: Peer) {
  if (prev.page === null) return false;
  if (next.page !== prev.page) return true;
  if (next.selection.length !== prev.selection.length || next.selection.some((id, i) => id !== prev.selection[i])) return true;
  return next.cursor !== null && (prev.cursor === null || next.cursor[0] !== prev.cursor[0] || next.cursor[1] !== prev.cursor[1]);
}

// ---------------- hooks ----------------

export function useLayers(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getLayers);
}
export function useComponents(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getComponents);
}
export function useStyles(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getStyles);
}
export function useSelection(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getSelection);
}
export function usePages(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getPages);
}
export function useUi(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getUi);
}
export function usePeers(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getPeers);
}
export function useComments(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getComments);
}
export function useCommentMode(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getCommentMode);
}
export function useOpenThread(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getOpenThread);
}
export function useFontState(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.fonts.getState);
}
export function useStatus(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getStatus);
}
export function useSaving(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getSaving);
}
export function usePaintTools(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getPaintTools);
}

/** Encode a 256×256 straight-alpha RGBA tile as PNG. */
async function tilePng(pixels: Uint8Array): Promise<Blob> {
  const size = 256;
  const data = new ImageData(new Uint8ClampedArray(pixels), size, size);
  if (typeof OffscreenCanvas !== 'undefined') {
    const c = new OffscreenCanvas(size, size);
    c.getContext('2d')!.putImageData(data, 0, 0);
    return c.convertToBlob({ type: 'image/png' });
  }
  const c = document.createElement('canvas');
  c.width = c.height = size;
  c.getContext('2d')!.putImageData(data, 0, 0);
  return new Promise((resolve, reject) => c.toBlob((b) => (b ? resolve(b) : reject(new Error('PNG encoding failed'))), 'image/png'));
}
