// Left panel: pages + a virtualized layer tree (front-most first), with
// selection, inline rename, visibility/lock toggles and drag-to-reorder.
import { useEffect, useMemo, useRef, useState, type DragEvent, type ReactNode } from 'react';
import { AssetsPanel } from './AssetsPanel';
import {
  Brush,
  ChevronDown,
  ChevronRight,
  Circle,
  Diamond,
  Component,
  Eye,
  EyeOff,
  Frame,
  Group,
  Hash,
  Lock,
  Minus,
  PenTool,
  Plus,
  Square,
  Star,
  Triangle,
  Type,
  Unlock,
} from 'lucide-react';
import { useMenu } from '../components/Menu';
import { useDialogs } from '../components/Dialogs';
import { useLayers, usePages, useSelection, useUi, type EditorStore } from './store';
import type { LayerNode, NodeType } from './types';

const ROW_H = 28;

function TypeIcon({ type, layout }: { type: NodeType; layout: boolean }) {
  const s = 13;
  switch (type) {
    case 'FRAME':
      return layout ? <Hash size={s} /> : <Frame size={s} />;
    case 'COMPONENT':
      return <Component size={s} />;
    case 'INSTANCE':
      return <Diamond size={s} />;
    case 'GROUP':
      return <Group size={s} />;
    case 'RECTANGLE':
      return <Square size={s} />;
    case 'ELLIPSE':
      return <Circle size={s} />;
    case 'TEXT':
      return <Type size={s} />;
    case 'LINE':
      return <Minus size={s} />;
    case 'VECTOR':
      return <PenTool size={s} />;
    case 'POLYGON':
      return <Triangle size={s} />;
    case 'STAR':
      return <Star size={s} />;
    case 'PAINT':
      return <Brush size={s} />;
    default:
      return <Component size={s} />;
  }
}

interface Row {
  node: LayerNode;
  depth: number;
  parent: string;
  /** Storage index among siblings (0 = back-most). */
  storageIndex: number;
  siblings: string[];
}

export function LayersPanel({ store }: { store: EditorStore }) {
  const [tab, setTab] = useState<'layers' | 'assets'>('layers');
  return (
    <div className="layers-panel">
      <div className="left-tabs" role="tablist">
        <button type="button" role="tab" aria-selected={tab === 'layers'} className={`panel-tab${tab === 'layers' ? ' active' : ''}`} onClick={() => setTab('layers')}>
          Layers
        </button>
        <button type="button" role="tab" aria-selected={tab === 'assets'} className={`panel-tab${tab === 'assets' ? ' active' : ''}`} onClick={() => setTab('assets')}>
          Assets
        </button>
      </div>
      {tab === 'layers' ? (
        <>
          <PagesSection store={store} />
          <LayerTree store={store} />
        </>
      ) : (
        <AssetsPanel store={store} />
      )}
    </div>
  );
}

function PagesSection({ store }: { store: EditorStore }) {
  const { pages, current } = usePages(store);
  const ui = useUi(store);
  const menu = useMenu();
  const dialogs = useDialogs();
  const [collapsed, setCollapsed] = useState(false);
  const engine = store.engine;
  return (
    <section className="pages">
      <header>
        <button type="button" className="section-toggle" onClick={() => setCollapsed((c) => !c)}>
          {collapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />} Pages
        </button>
        {ui.canEdit && (
          <button
            type="button"
            className="icon-btn"
            title="Add page"
            onClick={() => {
              engine.add_page(`Page ${pages.length + 1}`);
              store.poll();
            }}
          >
            <Plus size={14} />
          </button>
        )}
      </header>
      {!collapsed && (
        <ul>
          {pages.map((p) => (
            <li key={p.id}>
              <button
                type="button"
                className={`page-item${p.id === current ? ' active' : ''}`}
                onClick={() => {
                  engine.set_page(p.id);
                  store.poll();
                }}
                onDoubleClick={async () => {
                  if (!ui.canEdit) return;
                  const name = await dialogs.prompt({ title: 'Rename page', defaultValue: p.name, maxLength: 120 });
                  if (name) {
                    engine.rename_page(p.id, name);
                    store.poll();
                  }
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  if (!ui.canEdit) return;
                  menu.open({ x: e.clientX, y: e.clientY }, [
                    {
                      label: 'Rename',
                      onSelect: async () => {
                        const name = await dialogs.prompt({ title: 'Rename page', defaultValue: p.name, maxLength: 120 });
                        if (name) engine.rename_page(p.id, name);
                        store.poll();
                      },
                    },
                    {
                      label: 'Duplicate',
                      onSelect: () => {
                        engine.duplicate_page(p.id);
                        store.poll();
                      },
                    },
                    { type: 'separator' },
                    {
                      label: 'Delete',
                      danger: true,
                      disabled: pages.length <= 1,
                      onSelect: async () => {
                        const ok = await dialogs.confirm({ title: `Delete "${p.name}"?`, message: 'Everything on this page will be deleted.', danger: true, confirmLabel: 'Delete' });
                        if (ok) engine.delete_page(p.id);
                        store.poll();
                      },
                    },
                  ]);
                }}
              >
                {p.name}
              </button>
            </li>
          ))}
        </ul>
      )}
      {menu.element}
    </section>
  );
}

function LayerTree({ store }: { store: EditorStore }) {
  const layers = useLayers(store);
  const selection = useSelection(store);
  const ui = useUi(store);
  const engine = store.engine;
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [renaming, setRenaming] = useState<string | null>(null);
  const [scroll, setScroll] = useState({ top: 0, height: 600 });
  const [drop, setDrop] = useState<{ id: string; where: 'above' | 'below' | 'inside' } | null>(null);
  const dragId = useRef<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const anchor = useRef<string | null>(null);
  const selected = useMemo(() => new Set(selection.map((n) => n.id)), [selection]);

  // Parent map for auto-expanding to the selection.
  const parents = useMemo(() => {
    const m = new Map<string, string>();
    const walk = (nodes: LayerNode[], parent: string) => {
      for (const n of nodes) {
        m.set(n.id, parent);
        walk(n.children, n.id);
      }
    };
    walk(layers, ui.page);
    return m;
  }, [layers, ui.page]);

  useEffect(() => {
    if (!selection.length) return;
    setExpanded((prev) => {
      let changed = false;
      const next = new Set(prev);
      for (const n of selection) {
        let p = parents.get(n.id);
        while (p && p !== ui.page) {
          if (!next.has(p)) {
            next.add(p);
            changed = true;
          }
          p = parents.get(p);
        }
      }
      return changed ? next : prev;
    });
  }, [selection, parents, ui.page]);

  const rows = useMemo(() => {
    const out: Row[] = [];
    const walk = (nodes: LayerNode[], depth: number, parent: string) => {
      const storage = [...nodes].reverse().map((n) => n.id);
      nodes.forEach((n, displayIdx) => {
        out.push({ node: n, depth, parent, storageIndex: nodes.length - 1 - displayIdx, siblings: storage });
        if (n.children.length && expanded.has(n.id)) walk(n.children, depth + 1, n.id);
      });
    };
    walk(layers, 0, ui.page);
    return out;
  }, [layers, expanded, ui.page]);

  // Scroll the first selected row into view.
  useEffect(() => {
    const first = selection[0];
    const el = listRef.current;
    if (!first || !el) return;
    const i = rows.findIndex((r) => r.node.id === first.id);
    if (i < 0) return;
    const y = i * ROW_H;
    if (y < el.scrollTop || y + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = Math.max(0, y - el.clientHeight / 2);
  }, [selection, rows]);

  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setScroll((s) => ({ ...s, height: el.clientHeight })));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const first = Math.max(0, Math.floor(scroll.top / ROW_H) - 10);
  const last = Math.min(rows.length, Math.ceil((scroll.top + scroll.height) / ROW_H) + 10);

  const select = (id: string, e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) => {
    const ids = selection.map((n) => n.id);
    let next: string[];
    if (e.metaKey || e.ctrlKey) {
      next = ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id];
    } else if (e.shiftKey && anchor.current) {
      const a = rows.findIndex((r) => r.node.id === anchor.current);
      const b = rows.findIndex((r) => r.node.id === id);
      const [lo, hi] = a < b ? [a, b] : [b, a];
      next = rows.slice(lo, hi + 1).map((r) => r.node.id);
    } else {
      next = [id];
    }
    if (!e.shiftKey) anchor.current = id;
    engine.set_selection(JSON.stringify(next));
    store.poll();
  };

  const onDragOver = (e: DragEvent, row: Row) => {
    // Nothing can be dropped among an instance's layers.
    if (!dragId.current || dragId.current === row.node.id || row.node.derived) return;
    e.preventDefault();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const t = (e.clientY - r.top) / r.height;
    const container = row.node.type === 'FRAME' || row.node.type === 'GROUP' || row.node.type === 'COMPONENT';
    const where = container && t > 0.3 && t < 0.7 ? 'inside' : t < 0.5 ? 'above' : 'below';
    if (!drop || drop.id !== row.node.id || drop.where !== where) setDrop({ id: row.node.id, where });
  };

  const onDrop = (e: DragEvent, row: Row) => {
    e.preventDefault();
    const id = dragId.current;
    dragId.current = null;
    setDrop(null);
    if (!id || !drop || id === row.node.id) return;
    if (drop.where === 'inside') {
      engine.move_layer(id, row.node.id, 1_000_000);
    } else {
      const sibs = row.siblings.filter((s) => s !== id);
      const idx = sibs.indexOf(row.node.id);
      engine.move_layer(id, row.parent, drop.where === 'above' ? idx + 1 : idx);
    }
    store.poll();
  };

  return (
    <section className="layers">
      <header>
        <span>Layers</span>
      </header>
      <div className="layer-list" ref={listRef} onScroll={(e) => setScroll({ top: e.currentTarget.scrollTop, height: e.currentTarget.clientHeight })}>
        {rows.length === 0 && <div className="layers-empty">No layers yet. Pick a tool below or press F to draw a frame.</div>}
        <div style={{ height: rows.length * ROW_H, position: 'relative' }}>
          {rows.slice(first, last).map((row, k) => {
            const i = first + k;
            const n = row.node;
            const isSel = selected.has(n.id);
            const parentSelected = !isSel && isAncestorSelected(n.id, parents, selected);
            const dropClass = drop?.id === n.id ? ` drop-${drop.where}` : '';
            return (
              <div
                key={n.id}
                className={`layer-row${isSel ? ' selected' : ''}${parentSelected ? ' child-of-selected' : ''}${!n.visible ? ' hidden' : ''}${n.derived || n.type === 'COMPONENT' || n.type === 'INSTANCE' ? ' component-layer' : ''}${dropClass}`}
                style={{ top: i * ROW_H, paddingLeft: 8 + row.depth * 14 }}
                draggable={ui.canEdit && renaming !== n.id && !n.derived}
                onDragStart={(e) => {
                  dragId.current = n.id;
                  e.dataTransfer.effectAllowed = 'move';
                }}
                onDragOver={(e) => onDragOver(e, row)}
                onDragLeave={() => setDrop((d) => (d?.id === n.id ? null : d))}
                onDrop={(e) => onDrop(e, row)}
                onDragEnd={() => {
                  dragId.current = null;
                  setDrop(null);
                }}
                onMouseEnter={() => engine.hover_node(n.id)}
                onMouseLeave={() => engine.hover_node(undefined)}
                onClick={(e) => select(n.id, e)}
                onDoubleClick={() => {
                  if (ui.canEdit) setRenaming(n.id);
                }}
              >
                <button
                  type="button"
                  className="chevron"
                  style={{ visibility: n.children.length ? 'visible' : 'hidden' }}
                  onClick={(e) => {
                    e.stopPropagation();
                    setExpanded((prev) => {
                      const next = new Set(prev);
                      if (next.has(n.id)) next.delete(n.id);
                      else next.add(n.id);
                      return next;
                    });
                  }}
                  aria-label={expanded.has(n.id) ? 'Collapse' : 'Expand'}
                >
                  {expanded.has(n.id) ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                </button>
                <span className="type-icon">
                  <TypeIcon type={n.type} layout={n.layout} />
                </span>
                {renaming === n.id ? (
                  <RenameInput
                    initial={n.name}
                    onDone={(name) => {
                      setRenaming(null);
                      if (name && name !== n.name) {
                        engine.rename(n.id, name);
                        store.poll();
                      }
                    }}
                  />
                ) : (
                  <span className="layer-name">{n.name || n.type.toLowerCase()}</span>
                )}
                {ui.canEdit && (
                  <span className={`row-toggles${n.locked || !n.visible ? ' sticky' : ''}`}>
                    <ToggleBtn
                      title={n.locked ? 'Unlock' : 'Lock'}
                      show={n.locked}
                      onClick={() => {
                        engine.toggle_locked(JSON.stringify([n.id]));
                        store.poll();
                      }}
                    >
                      {n.locked ? <Lock size={12} /> : <Unlock size={12} />}
                    </ToggleBtn>
                    <ToggleBtn
                      title={n.visible ? 'Hide' : 'Show'}
                      show={!n.visible}
                      onClick={() => {
                        engine.toggle_visible(JSON.stringify([n.id]));
                        store.poll();
                      }}
                    >
                      {n.visible ? <Eye size={12} /> : <EyeOff size={12} />}
                    </ToggleBtn>
                  </span>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

function isAncestorSelected(id: string, parents: Map<string, string>, selected: Set<string>) {
  let p = parents.get(id);
  while (p) {
    if (selected.has(p)) return true;
    p = parents.get(p);
  }
  return false;
}

function ToggleBtn({ title, show, onClick, children }: { title: string; show: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      className={`row-toggle${show ? ' show' : ''}`}
      title={title}
      aria-label={title}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
    >
      {children}
    </button>
  );
}

function RenameInput({ initial, onDone }: { initial: string; onDone: (name: string | null) => void }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  return (
    <input
      ref={ref}
      className="rename-input"
      defaultValue={initial}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Enter') onDone(e.currentTarget.value.trim() || null);
        if (e.key === 'Escape') onDone(null);
      }}
      onBlur={(e) => onDone(e.currentTarget.value.trim() || null)}
    />
  );
}
