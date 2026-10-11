// The canvas host: forwards input to the wasm engine, runs the render loop,
// syncs over the socket, and hosts DOM overlays (peer cursors, text editing).
import { useEffect, useRef, type MouseEvent as ReactMouseEvent } from 'react';
import type { EditorStore } from './store';
import type { Peer } from './types';
import { TextEditOverlay } from './TextEditOverlay';
import { CommentsLayer } from './CommentsLayer';

interface Props {
  store: EditorStore;
  canvas: HTMLCanvasElement;
  onContextMenu: (e: ReactMouseEvent<HTMLDivElement>) => void;
  onDropFiles: (files: File[], x: number, y: number) => void;
}

function isTypingTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  const tag = t.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || t.isContentEditable;
}

/** Imperative peer-cursor layer: positioned every frame without React renders. */
class CursorLayer {
  private els = new Map<string, HTMLDivElement>();
  constructor(private root: HTMLDivElement) {}

  sync(peers: Peer[], page: string) {
    const live = new Set<string>();
    for (const p of peers) {
      if (!p.cursor || p.page !== page) continue;
      live.add(p.clientId);
      let el = this.els.get(p.clientId);
      if (!el) {
        el = document.createElement('div');
        el.className = 'peer-cursor' + (p.isAgent ? ' agent' : '');
        el.innerHTML =
          '<svg width="18" height="18" viewBox="0 0 18 18"><path d="M2 1.5 L15.5 8.2 L9.2 9.6 L6.3 15.8 Z" stroke="white" stroke-width="1.4" stroke-linejoin="round"/></svg><span></span>';
        this.root.appendChild(el);
        this.els.set(p.clientId, el);
      }
      const path = el.querySelector('path');
      if (path) path.setAttribute('fill', p.color);
      const label = el.querySelector('span');
      if (label) {
        label.textContent = p.isAgent ? `✦ ${p.name}` : p.name;
        label.style.background = p.color;
      }
      el.dataset.x = String(p.cursor[0]);
      el.dataset.y = String(p.cursor[1]);
    }
    for (const [id, el] of this.els) {
      if (!live.has(id)) {
        el.remove();
        this.els.delete(id);
      }
    }
  }

  position(cam: Float64Array | number[]) {
    const [cx, cy, z] = cam;
    for (const el of this.els.values()) {
      const x = Number(el.dataset.x) * z + cx;
      const y = Number(el.dataset.y) * z + cy;
      el.style.transform = `translate(${x}px, ${y}px)`;
    }
  }
}

export function Canvas({ store, canvas, onContextMenu, onDropFiles }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const cursorsRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current!;
    canvas.className = 'canvas';
    canvas.tabIndex = -1;
    host.prepend(canvas);
    const engine = store.engine;
    const cursors = new CursorLayer(cursorsRef.current!);
    let raf = 0;
    let lastCursor = '';
    let lastCam = '';
    let peersRef: Peer[] = [];
    let pageRef = '';

    const local = (e: { clientX: number; clientY: number }) => {
      const r = canvas.getBoundingClientRect();
      return [e.clientX - r.left, e.clientY - r.top] as const;
    };
    const meta = (e: { metaKey: boolean; ctrlKey: boolean }) => e.metaKey || e.ctrlKey;

    const updateCursor = () => {
      const c = engine.cursor();
      if (c !== lastCursor) {
        canvas.style.cursor = c;
        lastCursor = c;
      }
    };

    // ---- sizing ----
    const resize = () => {
      const r = host.getBoundingClientRect();
      const dpr = Math.min(window.devicePixelRatio || 1, 3);
      // Cap the back buffer at ~16 MP.
      const scale = Math.min(dpr, Math.sqrt(16_000_000 / Math.max(1, r.width * r.height)));
      canvas.width = Math.max(1, Math.round(r.width * scale));
      canvas.height = Math.max(1, Math.round(r.height * scale));
      canvas.style.width = `${r.width}px`;
      canvas.style.height = `${r.height}px`;
      engine.resize(r.width, r.height, scale);
    };
    const ro = new ResizeObserver(resize);
    ro.observe(host);
    resize();

    // ---- touch: two-finger pinch-zoom and pan ----
    const touches = new Map<number, { x: number; y: number }>();
    let pinch: { dist: number; cx: number; cy: number } | null = null;
    const pinchState = () => {
      const [a, b] = [...touches.values()];
      return { dist: Math.hypot(a.x - b.x, a.y - b.y), cx: (a.x + b.x) / 2, cy: (a.y + b.y) / 2 };
    };
    const touchDown = (e: PointerEvent): boolean => {
      if (e.pointerType !== 'touch') return false;
      const [x, y] = local(e);
      touches.set(e.pointerId, { x, y });
      if (touches.size === 2) {
        engine.cancel_gesture();
        pinch = pinchState();
        return true;
      }
      return touches.size > 2;
    };
    const touchMove = (e: PointerEvent): boolean => {
      if (e.pointerType !== 'touch' || !touches.has(e.pointerId)) return false;
      const [x, y] = local(e);
      touches.set(e.pointerId, { x, y });
      if (!pinch || touches.size < 2) return false;
      const next = pinchState();
      engine.wheel(-(next.cx - pinch.cx), -(next.cy - pinch.cy), next.cx, next.cy, false);
      if (pinch.dist > 0) engine.pinch(next.dist / pinch.dist, next.cx, next.cy);
      pinch = next;
      return true;
    };
    const touchUp = (e: PointerEvent): boolean => {
      if (e.pointerType !== 'touch') return false;
      const wasPinching = pinch !== null;
      touches.delete(e.pointerId);
      if (touches.size < 2) pinch = null;
      return wasPinching;
    };

    // ---- pointer ----
    const onPointerDown = (e: PointerEvent) => {
      if (e.button === 2) return;
      if (touchDown(e)) return;
      canvas.setPointerCapture(e.pointerId);
      const [x, y] = local(e);
      engine.pointer_down(x, y, e.button, e.shiftKey, e.altKey, meta(e));
      updateCursor();
      store.poll();
      if (e.button === 1) e.preventDefault();
    };
    const onPointerMove = (e: PointerEvent) => {
      if (touchMove(e)) return;
      const [x, y] = local(e);
      engine.pointer_move(x, y, e.shiftKey, e.altKey, meta(e));
      const w = engine.screen_to_world(x, y);
      store.setCursor([Math.round(w[0] * 10) / 10, Math.round(w[1] * 10) / 10]);
      updateCursor();
    };
    const onPointerUp = (e: PointerEvent) => {
      if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
      if (touchUp(e)) return;
      const [x, y] = local(e);
      engine.pointer_up(x, y, e.shiftKey, e.altKey, meta(e));
      updateCursor();
      store.poll();
    };
    const onPointerLeave = () => {
      engine.pointer_leave();
      store.setCursor(null);
    };
    const onDblClick = (e: MouseEvent) => {
      const [x, y] = local(e);
      engine.double_click(x, y);
      store.poll();
    };
    const onAuxClick = (e: MouseEvent) => {
      if (e.button === 1) e.preventDefault();
    };

    // ---- wheel / pinch ----
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const [x, y] = local(e);
      const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 400 : 1;
      const zoom = e.ctrlKey || e.metaKey;
      let dx = e.deltaX * k;
      let dy = e.deltaY * k;
      if (!zoom && e.shiftKey && dx === 0) {
        dx = dy;
        dy = 0;
      }
      engine.wheel(dx, dy, x, y, zoom);
    };
    let gestureScale = 1;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      gestureScale = 1;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const ge = e as Event & { scale: number; clientX: number; clientY: number };
      const [x, y] = local(ge);
      engine.pinch(ge.scale / gestureScale, x, y);
      gestureScale = ge.scale;
    };

    // ---- keyboard ----
    const onKeyDown = (e: KeyboardEvent) => {
      if (isTypingTarget(e.target)) return;
      if (!e.metaKey && !e.ctrlKey && !e.altKey && e.key.toLowerCase() === 'c') {
        e.preventDefault();
        store.setCommentMode(!store.getCommentMode());
        return;
      }
      if (e.key === 'Escape' && store.getCommentMode()) {
        e.preventDefault();
        if (store.getOpenThread()) store.setOpenThread(null);
        else store.setCommentMode(false);
        return;
      }
      if (store.getCommentMode() && !e.metaKey && !e.ctrlKey && e.key.length === 1) store.setCommentMode(false);
      if ((e.metaKey || e.ctrlKey) && ['c', 'x', 'v'].includes(e.key.toLowerCase())) return; // clipboard events
      if ((e.metaKey || e.ctrlKey) && ['s', 'p'].includes(e.key.toLowerCase())) {
        e.preventDefault();
        return;
      }
      if (engine.key_down(e.key, e.shiftKey, e.altKey, e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        updateCursor();
        store.poll();
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (isTypingTarget(e.target)) return;
      engine.key_up(e.key, e.shiftKey, e.altKey, e.metaKey || e.ctrlKey);
      updateCursor();
    };
    const onBlur = () => engine.key_up(' ', false, false, false);

    // ---- clipboard ----
    const CLIP_PREFIX = 'flexma-clipboard:';
    const onCopy = (e: ClipboardEvent) => {
      if (isTypingTarget(e.target)) return;
      const data = engine.copy();
      if (!data || !e.clipboardData) return;
      e.preventDefault();
      e.clipboardData.setData('text/plain', CLIP_PREFIX + data);
    };
    const onCut = (e: ClipboardEvent) => {
      if (isTypingTarget(e.target)) return;
      const data = engine.cut();
      if (!data || !e.clipboardData) return;
      e.preventDefault();
      e.clipboardData.setData('text/plain', CLIP_PREFIX + data);
      store.poll();
    };
    const onPaste = (e: ClipboardEvent) => {
      if (isTypingTarget(e.target) || !e.clipboardData) return;
      const files = Array.from(e.clipboardData.files).filter((f) => f.type.startsWith('image/'));
      if (files.length) {
        e.preventDefault();
        onDropFiles(files, -1, -1);
        return;
      }
      const text = e.clipboardData.getData('text/plain');
      if (text.startsWith(CLIP_PREFIX)) {
        e.preventDefault();
        engine.paste(text.slice(CLIP_PREFIX.length));
        store.poll();
      }
    };

    // ---- drag & drop images ----
    const onDragOver = (e: DragEvent) => {
      if (e.dataTransfer?.types.includes('Files')) e.preventDefault();
    };
    const onDrop = (e: DragEvent) => {
      const files = Array.from(e.dataTransfer?.files ?? []).filter((f) => f.type.startsWith('image/'));
      if (!files.length) return;
      e.preventDefault();
      const [x, y] = local(e);
      onDropFiles(files, x, y);
    };

    // ---- GPU context loss: keep the page alive, then reload to rebuild GPU state ----
    const onContextLost = (e: Event) => e.preventDefault();
    const onContextRestored = () => window.location.reload();
    canvas.addEventListener('webglcontextlost', onContextLost);
    canvas.addEventListener('webglcontextrestored', onContextRestored);

    // ---- fonts ----
    document.fonts?.ready.then(() => engine.fonts_ready());
    const onFonts = () => engine.fonts_ready();
    document.fonts?.addEventListener?.('loadingdone', onFonts);

    // ---- render loop ----
    const unsubscribe = store.subscribe(() => {
      peersRef = store.getPeers();
      pageRef = store.getUi().page;
      cursors.sync(peersRef, pageRef);
      lastCam = '';
    });
    const loop = (t: number) => {
      engine.frame(t);
      store.flush();
      store.poll();
      const cam = engine.camera();
      const key = `${cam[0]},${cam[1]},${cam[2]}`;
      if (key !== lastCam) {
        lastCam = key;
        cursors.position(cam);
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);

    canvas.addEventListener('pointerdown', onPointerDown);
    canvas.addEventListener('pointermove', onPointerMove);
    canvas.addEventListener('pointerup', onPointerUp);
    canvas.addEventListener('pointercancel', onPointerUp);
    canvas.addEventListener('pointerleave', onPointerLeave);
    canvas.addEventListener('dblclick', onDblClick);
    canvas.addEventListener('auxclick', onAuxClick);
    canvas.addEventListener('wheel', onWheel, { passive: false });
    canvas.addEventListener('gesturestart', onGestureStart as EventListener);
    canvas.addEventListener('gesturechange', onGestureChange as EventListener);
    host.addEventListener('dragover', onDragOver);
    host.addEventListener('drop', onDrop);
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('keyup', onKeyUp);
    window.addEventListener('blur', onBlur);
    document.addEventListener('copy', onCopy);
    document.addEventListener('cut', onCut);
    document.addEventListener('paste', onPaste);

    return () => {
      cancelAnimationFrame(raf);
      unsubscribe();
      ro.disconnect();
      canvas.removeEventListener('pointerdown', onPointerDown);
      canvas.removeEventListener('pointermove', onPointerMove);
      canvas.removeEventListener('pointerup', onPointerUp);
      canvas.removeEventListener('pointercancel', onPointerUp);
      canvas.removeEventListener('pointerleave', onPointerLeave);
      canvas.removeEventListener('dblclick', onDblClick);
      canvas.removeEventListener('auxclick', onAuxClick);
      canvas.removeEventListener('wheel', onWheel);
      canvas.removeEventListener('gesturestart', onGestureStart as EventListener);
      canvas.removeEventListener('gesturechange', onGestureChange as EventListener);
      host.removeEventListener('dragover', onDragOver);
      host.removeEventListener('drop', onDrop);
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('keyup', onKeyUp);
      window.removeEventListener('blur', onBlur);
      document.removeEventListener('copy', onCopy);
      document.removeEventListener('cut', onCut);
      document.removeEventListener('paste', onPaste);
      document.fonts?.removeEventListener?.('loadingdone', onFonts);
      canvas.removeEventListener('webglcontextlost', onContextLost);
      canvas.removeEventListener('webglcontextrestored', onContextRestored);
    };
  }, [store, canvas, onDropFiles]);

  return (
    <div className="canvas-host" ref={hostRef} onContextMenu={onContextMenu}>
      <div className="peer-cursors" ref={cursorsRef} />
      <TextEditOverlay store={store} />
      <CommentsLayer store={store} />
    </div>
  );
}
