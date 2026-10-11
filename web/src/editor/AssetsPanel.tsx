// Assets: the file's shared styles and main components. Click a style to
// apply it to the selection; click a component to place an instance.
import { useMemo, useState } from 'react';
import { Component as ComponentIcon, Crosshair, Pencil, Plus } from 'lucide-react';
import { useComponents, useSelection, useStyles, useUi, type EditorStore } from './store';
import { StyleEditor, StyleSwatch, textStyleSummary } from './Styles';
import type { StyleInfo } from './types';

export function AssetsPanel({ store }: { store: EditorStore }) {
  const components = useComponents(store);
  const ui = useUi(store);
  const [q, setQ] = useState('');
  // Thumbnails refresh when the panel is opened (not on every edit).
  const [stamp] = useState(() => Date.now());
  const list = useMemo(
    () => components.filter((c) => !q || c.name.toLowerCase().includes(q.toLowerCase())),
    [components, q],
  );
  const byPage = useMemo(() => {
    const m = new Map<string, typeof list>();
    for (const c of list) {
      const k = c.pageName ?? 'Page';
      m.set(k, [...(m.get(k) ?? []), c]);
    }
    return [...m.entries()];
  }, [list]);

  return (
    <div className="assets">
      <input className="prop-text assets-search" placeholder="Search styles and components" value={q} onChange={(e) => setQ(e.target.value)} />
      <StylesList store={store} query={q} />
      {components.length === 0 ? (
        <div className="assets-empty">
          <ComponentIcon size={22} />
          <p>
            No components yet. Select a frame and press <kbd className="kbd">⌥⌘K</kbd> to make it a reusable component.
          </p>
        </div>
      ) : (
        byPage.map(([page, items]) => (
          <section key={page} className="assets-group">
            <h4>{page}</h4>
            <ul>
              {items.map((c) => (
                <li key={c.id} className="asset-item">
                  <button
                    type="button"
                    className="asset-thumb"
                    title={ui.canEdit ? `Insert “${c.name}”` : c.name}
                    disabled={!ui.canEdit}
                    onClick={() => {
                      store.engine.insert_instance(c.id);
                      store.poll();
                    }}
                  >
                    <img src={`${store.exportSvgUrl({ node: c.id })}&background=false&v=${stamp}`} alt="" loading="lazy" />
                  </button>
                  <div className="asset-meta">
                    <span className="asset-name">{c.name}</span>
                    <span className="text-muted">
                      {Math.round(c.width)}×{Math.round(c.height)} · {c.instances} instance{c.instances === 1 ? '' : 's'}
                    </span>
                  </div>
                  <div className="asset-actions">
                    {ui.canEdit && (
                      <button
                        type="button"
                        className="icon-btn"
                        title="Insert instance"
                        onClick={() => {
                          store.engine.insert_instance(c.id);
                          store.poll();
                        }}
                      >
                        <Plus size={14} />
                      </button>
                    )}
                    <button
                      type="button"
                      className="icon-btn"
                      title="Go to main component"
                      onClick={() => {
                        if (c.page && c.page !== ui.page) store.engine.set_page(c.page);
                        store.engine.set_selection(JSON.stringify([c.id]));
                        store.engine.zoom_to_selection();
                        store.poll();
                      }}
                    >
                      <Crosshair size={14} />
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          </section>
        ))
      )}
    </div>
  );
}

function StylesList({ store, query }: { store: EditorStore; query: string }) {
  const styles = useStyles(store);
  const selection = useSelection(store);
  const ui = useUi(store);
  const [editing, setEditing] = useState<StyleInfo | null>(null);
  const q = query.trim().toLowerCase();
  const shown = styles.filter((s) => !q || s.name.toLowerCase().includes(q));
  if (styles.length === 0) {
    return (
      <p className="assets-hint">
        No styles yet. Use the swatch button in the Fill or Text section to turn a color or text look into a shared style.
      </p>
    );
  }
  const apply = (s: StyleInfo) => {
    if (!ui.canEdit || selection.length === 0) {
      setEditing(s);
      return;
    }
    store.engine.apply_style(s.type === 'TEXT' ? 'text' : 'fill', s.id);
    store.poll();
  };
  const groups: [string, StyleInfo[]][] = [
    ['Color styles', shown.filter((s) => s.type === 'FILL')],
    ['Text styles', shown.filter((s) => s.type === 'TEXT')],
  ];
  return (
    <>
      {groups
        .filter(([, list]) => list.length > 0)
        .map(([label, list]) => (
          <section key={label} className="assets-group">
            <h4>{label}</h4>
            <ul className="style-list">
              {list.map((s) => (
                <li key={s.id} className="style-row">
                  <button
                    type="button"
                    className="style-row-main"
                    onClick={() => apply(s)}
                    title={selection.length && ui.canEdit ? `Apply “${s.name}” to the selection` : s.description || s.name}
                  >
                    <StyleSwatch store={store} style={s} />
                    <span className="style-row-name">
                      {s.name}
                      <small>{s.type === 'TEXT' ? textStyleSummary(s) : `${s.uses} layer${s.uses === 1 ? '' : 's'}`}</small>
                    </span>
                  </button>
                  {ui.canEdit && (
                    <button type="button" className="icon-btn" title="Edit style" onClick={() => setEditing(s)}>
                      <Pencil size={13} />
                    </button>
                  )}
                </li>
              ))}
            </ul>
          </section>
        ))}
      {editing && <StyleEditor store={store} style={editing} onClose={() => setEditing(null)} />}
    </>
  );
}
