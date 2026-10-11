// Assets: the file's main components. Click to place an instance; instances
// follow the component and keep their own overrides.
import { useMemo, useState } from 'react';
import { Component as ComponentIcon, Crosshair, Plus } from 'lucide-react';
import { useComponents, useUi, type EditorStore } from './store';

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
      <input className="prop-text assets-search" placeholder="Search components" value={q} onChange={(e) => setQ(e.target.value)} />
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
