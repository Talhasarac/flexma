// EditorStore: the bridge between the wasm engine, the multiplayer socket and
// React. React subscribes with useSyncExternalStore; snapshots are cached per
// engine version counter so components re-render only when their slice changes.
import { useSyncExternalStore } from 'react';
import type { FlexmaEditor } from '../engine';
import { api, exportPngUrl, exportSvgUrl, shareUrls, type CommentThread, type ExportOptions, type PngExportOptions } from '../lib/api';
import type { ComponentInfo, ConnectionStatus, EditingText, LayerNode, NodeData, PageInfo, Peer, Tool } from './types';

type Listener = () => void;

const SEND_INTERVAL_MS = 33; // ~30 batches per second (Figma's cadence)
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

  constructor(engine: FlexmaEditor, fileId: string, shareToken: string | null = null) {
    this.engine = engine;
    this.fileId = fileId;
    this.shareToken = shareToken;
    // Guests have no session cookie, so images load through the link instead.
    if (shareToken) engine.set_asset_base(shareUrls.assetBase(shareToken));
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
      this.pages = JSON.parse(e.pages()) as { pages: PageInfo[]; current: string };
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
      this.peers = [];
      this.engine.set_peers('[]');
      this.notify();
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
        this.peers = ((msg.peers as Peer[]) ?? []).filter((p) => p.clientId !== this.clientId);
        this.engine.set_peers(JSON.stringify(this.peers));
        this.status = 'online';
        this.presenceDirty = true;
        void this.refreshComments();
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
        const i = this.peers.findIndex((p) => p.clientId === peer.clientId);
        this.peers = i >= 0 ? this.peers.map((p, j) => (j === i ? peer : p)) : [...this.peers, peer];
        this.engine.set_peers(JSON.stringify(this.peers));
        this.notify();
        break;
      }
      case 'leave': {
        this.peers = this.peers.filter((p) => p.clientId !== msg.clientId);
        this.engine.set_peers(JSON.stringify(this.peers));
        this.notify();
        break;
      }
      case 'comments':
        void this.refreshComments();
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
    }
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

  setCursor(world: [number, number] | null) {
    this.cursorWorld = world;
    this.presenceDirty = true;
  }
}

// ---------------- hooks ----------------

export function useLayers(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getLayers);
}
export function useComponents(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getComponents);
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
export function useStatus(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getStatus);
}
export function useSaving(s: EditorStore) {
  return useSyncExternalStore(s.subscribe, s.getSaving);
}
