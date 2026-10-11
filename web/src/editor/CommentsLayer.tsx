// Comment pins on the canvas. Pins live in world space (positioned every frame
// from the engine camera; pins attached to a node follow it), with a thread
// popover for replies, resolve and delete. In comment mode a click anywhere on
// the canvas starts a new thread.
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import { Check, Sparkles, Trash2, X } from 'lucide-react';
import { api, type CommentThread } from '../lib/api';
import { useAuth } from '../lib/auth';
import { useToast } from '../components/Toast';
import { formatRelativeTime } from '../lib/format';
import { Avatar } from '../components/Avatar';
import { useCommentMode, useComments, useOpenThread, useUi, type EditorStore } from './store';

interface Draft {
  x: number;
  y: number;
  nodeId: string | null;
  pageId: string;
}

/** World position of a thread's pin. Node-attached pins are relative to the node. */
export function pinPosition(store: EditorStore, t: { nodeId: string | null; x: number; y: number }): [number, number] {
  if (t.nodeId) {
    const b = store.engine.node_bounds(t.nodeId);
    if (b.length) return [b[0] + t.x, b[1] + t.y];
  }
  return [t.x, t.y];
}

export function CommentsLayer({ store }: { store: EditorStore }) {
  const threads = useComments(store);
  const mode = useCommentMode(store);
  const open = useOpenThread(store);
  const ui = useUi(store);
  const layerRef = useRef<HTMLDivElement>(null);
  const [draft, setDraft] = useState<Draft | null>(null);

  // Position pins every frame (camera moves, nodes move).
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const layer = layerRef.current;
      if (layer) {
        const [cx, cy, z] = store.engine.camera();
        layer.querySelectorAll<HTMLElement>('[data-anchor]').forEach((el) => {
          const node = el.dataset.node || null;
          const [wx, wy] = pinPosition(store, { nodeId: node, x: Number(el.dataset.x), y: Number(el.dataset.y) });
          el.style.transform = `translate(${wx * z + cx}px, ${wy * z + cy}px)`;
        });
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [store]);

  useEffect(() => {
    if (!mode) setDraft(null);
  }, [mode]);

  const startDraft = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget || e.button !== 0) return;
    const r = e.currentTarget.getBoundingClientRect();
    const sx = e.clientX - r.left;
    const sy = e.clientY - r.top;
    const [wx, wy] = store.engine.screen_to_world(sx, sy);
    const nodeId = store.engine.hit_test(sx, sy, true) ?? null;
    let x = wx;
    let y = wy;
    if (nodeId) {
      const b = store.engine.node_bounds(nodeId);
      if (b.length) {
        x = wx - b[0];
        y = wy - b[1];
      }
    }
    setDraft({ x, y, nodeId, pageId: ui.page });
    store.setOpenThread('draft');
  };

  const visible = threads.filter((t) => (t.pageId ?? ui.page) === ui.page && (!t.resolved || open === t.id));

  return (
    <div ref={layerRef} className={`comments-layer${mode ? ' capture' : ''}`} onPointerDown={mode ? startDraft : undefined}>
      {visible.map((t) => (
        <div key={t.id} className="comment-anchor" data-anchor data-x={t.x} data-y={t.y} data-node={t.nodeId ?? ''}>
          <button
            type="button"
            className={`comment-pin${open === t.id ? ' open' : ''}${t.resolved ? ' resolved' : ''}`}
            style={{ ['--pin' as string]: t.author.color }}
            title={`${t.author.name}: ${t.body}`}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={() => store.setOpenThread(open === t.id ? null : t.id)}
          >
            {t.author.isAgent ? <Sparkles size={12} /> : <span>{(t.author.name.trim()[0] ?? '?').toUpperCase()}</span>}
            {t.replies.length > 0 && <em>{t.replies.length + 1}</em>}
          </button>
          {open === t.id && <ThreadPopover store={store} thread={t} onClose={() => store.setOpenThread(null)} />}
        </div>
      ))}
      {draft && open === 'draft' && (
        <div className="comment-anchor" data-anchor data-x={draft.x} data-y={draft.y} data-node={draft.nodeId ?? ''}>
          <span className="comment-pin draft" />
          <DraftPopover
            store={store}
            draft={draft}
            onDone={() => {
              setDraft(null);
              store.setOpenThread(null);
            }}
          />
        </div>
      )}
    </div>
  );
}

function Composer({ placeholder, onSubmit, autoFocus }: { placeholder: string; onSubmit: (text: string) => Promise<void>; autoFocus?: boolean }) {
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    const body = text.trim();
    if (!body || busy) return;
    setBusy(true);
    try {
      await onSubmit(body);
      setText('');
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="comment-composer">
      <textarea
        value={text}
        placeholder={placeholder}
        autoFocus={autoFocus}
        rows={2}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            void submit();
          }
        }}
      />
      <button type="button" className="btn btn-primary btn-xs" disabled={!text.trim() || busy} onClick={() => void submit()}>
        Post
      </button>
    </div>
  );
}

function DraftPopover({ store, draft, onDone }: { store: EditorStore; draft: Draft; onDone: () => void }) {
  const toast = useToast();
  return (
    <div className="comment-popover" onPointerDown={(e) => e.stopPropagation()}>
      <header>
        <span>New comment</span>
        <button type="button" className="icon-btn" aria-label="Cancel" onClick={onDone}>
          <X size={14} />
        </button>
      </header>
      <Composer
        autoFocus
        placeholder="Add a comment… (agents can read and resolve these)"
        onSubmit={async (body) => {
          try {
            await api.comments.create(store.fileId, { body, pageId: draft.pageId, nodeId: draft.nodeId, x: draft.x, y: draft.y });
            await store.refreshComments();
            onDone();
          } catch (e) {
            toast.error(e);
          }
        }}
      />
    </div>
  );
}

function ThreadPopover({ store, thread, onClose }: { store: EditorStore; thread: CommentThread; onClose: () => void }) {
  const toast = useToast();
  const { user } = useAuth();
  const all = [thread, ...thread.replies];
  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
      await store.refreshComments();
    } catch (e) {
      toast.error(e);
    }
  };
  return (
    <div className="comment-popover" onPointerDown={(e) => e.stopPropagation()} onWheel={(e) => e.stopPropagation()}>
      <header>
        <span>{thread.resolved ? 'Resolved thread' : 'Comment'}</span>
        <div className="row-actions">
          <button
            type="button"
            className={`icon-btn${thread.resolved ? ' active' : ''}`}
            title={thread.resolved ? 'Reopen' : 'Resolve'}
            onClick={() =>
              void run(async () => {
                await api.comments.update(thread.id, { resolved: !thread.resolved });
                if (!thread.resolved) onClose();
              })
            }
          >
            <Check size={14} />
          </button>
          <button type="button" className="icon-btn" aria-label="Close" onClick={onClose}>
            <X size={14} />
          </button>
        </div>
      </header>
      <ol className="comment-messages">
        {all.map((c) => (
          <li key={c.id}>
            <Avatar name={c.author.name} color={c.author.color} size={22} />
            <div className="comment-body">
              <div className="comment-meta">
                <b>{c.author.name}</b>
                {c.author.isAgent && <span className="agent-tag">AI</span>}
                <span>{formatRelativeTime(c.createdAt)}</span>
                {user && c.author.id === user.id && !c.author.isAgent && (
                  <button
                    type="button"
                    className="icon-btn comment-delete"
                    title="Delete"
                    onClick={() =>
                      void run(async () => {
                        await api.comments.delete(c.id);
                        if (c.id === thread.id) onClose();
                      })
                    }
                  >
                    <Trash2 size={12} />
                  </button>
                )}
              </div>
              <p>{c.body}</p>
            </div>
          </li>
        ))}
      </ol>
      <Composer placeholder="Reply…" onSubmit={(body) => run(() => api.comments.create(store.fileId, { body, parentId: thread.id }))} />
    </div>
  );
}

/** Right-panel list of threads shown in comment mode. */
export function CommentsPanel({ store }: { store: EditorStore }) {
  const threads = useComments(store);
  const ui = useUi(store);
  const [showResolved, setShowResolved] = useState(false);
  const list = threads.filter((t) => showResolved || !t.resolved).slice().reverse();
  return (
    <div className="comments-panel">
      <div className="comments-panel-head">
        <span>{list.length ? `${list.length} thread${list.length === 1 ? '' : 's'}` : 'No comments yet'}</span>
        <label className="check">
          <input type="checkbox" checked={showResolved} onChange={(e) => setShowResolved(e.target.checked)} /> Show resolved
        </label>
      </div>
      <p className="comments-hint">Click anywhere on the canvas to comment. Press C to toggle comment mode, Esc to exit.</p>
      <ul>
        {list.map((t) => (
          <li key={t.id}>
            <button
              type="button"
              className={`comment-row${t.resolved ? ' resolved' : ''}`}
              onClick={() => {
                if (t.pageId && t.pageId !== ui.page) store.engine.set_page(t.pageId);
                const [x, y] = pinPosition(store, t);
                store.engine.center_on(x, y);
                store.setOpenThread(t.id);
                store.poll();
              }}
            >
              <Avatar name={t.author.name} color={t.author.color} size={22} />
              <span className="comment-row-text">
                <span className="comment-meta">
                  <b>{t.author.name}</b> <span>{formatRelativeTime(t.createdAt)}</span>
                </span>
                <span className="comment-row-body">{t.body}</span>
                {t.replies.length > 0 && <span className="comment-replies">{t.replies.length} repl{t.replies.length === 1 ? 'y' : 'ies'}</span>}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
