// Shared color and text styles: swatches, the style picker menu, the chip a
// linked layer shows, and the style editor dialog.
import { useEffect, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { Pencil, SwatchBook, Trash, Unlink } from 'lucide-react';
import { Modal } from '../components/Modal';
import { useDialogs } from '../components/Dialogs';
import { useMenu, type MenuEntry } from '../components/Menu';
import { weightName, WEIGHTS } from '../lib/fonts';
import { FontPicker } from './FontPicker';
import { ColorInput, IconButton, NumberInput, Select } from './inputs';
import { useStyles, useUi, type EditorStore } from './store';
import type { Paint, StyleInfo, StyleKind } from './types';

/** CSS background for a list of paints (top paint last). */
export function paintsCss(paints: Paint[], assetUrl: (id: string) => string): string {
  const layers = [...paints]
    .filter((p) => p.visible !== false)
    .reverse()
    .map((p) => {
      if (p.type === 'SOLID') return `linear-gradient(${p.color}, ${p.color})`;
      if (p.type === 'LINEAR_GRADIENT') return `linear-gradient(${(p.angle ?? 0) + 90}deg, ${p.stops.map((s) => `${s.color} ${s.position * 100}%`).join(', ')})`;
      return `url(${assetUrl(p.asset)}) center / cover`;
    });
  return layers.length ? layers.join(', ') : 'transparent';
}

export function StyleSwatch({ store, style, size = 16 }: { store: EditorStore; style: StyleInfo; size?: number }) {
  if (style.type === 'TEXT') {
    return (
      <span className="style-aa" style={{ fontFamily: `"${style.fontFamily}", Inter, sans-serif`, fontWeight: style.fontWeight, fontStyle: style.italic ? 'italic' : undefined }}>
        Ag
      </span>
    );
  }
  return <span className="style-swatch" style={{ width: size, height: size, background: paintsCss(style.fills, (id) => store.assetUrl(id)) }} />;
}

export function textStyleSummary(s: StyleInfo): string {
  const lh = s.lineHeight ? `/${Math.round(s.lineHeight * 10) / 10}` : '';
  return `${s.fontFamily} ${Math.round(s.fontSize * 10) / 10}${lh} · ${weightName(s.fontWeight)}${s.italic ? ' italic' : ''}`;
}

/** Opens the style menu for a kind: apply an existing style or create one. */
export function useStylePicker(store: EditorStore) {
  const menu = useMenu();
  const dialogs = useDialogs();
  const styles = useStyles(store);
  const open = (anchor: HTMLElement | { x: number; y: number }, kind: StyleKind) => {
    const want = kind === 'text' ? 'TEXT' : 'FILL';
    const list = styles.filter((s) => s.type === want);
    const entries: MenuEntry[] = [];
    if (list.length) entries.push({ type: 'header', label: kind === 'text' ? 'Text styles' : 'Color styles' });
    for (const s of list) {
      entries.push({
        label: s.name,
        icon: <StyleSwatch store={store} style={s} size={14} />,
        hint: s.type === 'TEXT' ? `${Math.round(s.fontSize)}` : undefined,
        onSelect: () => {
          store.engine.apply_style(kind, s.id);
          store.poll();
        },
      });
    }
    if (list.length) entries.push({ type: 'separator' });
    entries.push({
      label: kind === 'text' ? 'Create text style from selection…' : `Create color style from ${kind === 'stroke' ? 'stroke' : 'fill'}…`,
      onSelect: async () => {
        const name = await dialogs.prompt({
          title: kind === 'text' ? 'New text style' : 'New color style',
          message: 'Use "/" to group styles, e.g. Brand/Primary or Heading/H1. Everything using the style updates when you change it.',
          label: 'Style name',
          placeholder: kind === 'text' ? 'Heading/H1' : 'Brand/Primary',
          confirmLabel: 'Create style',
          maxLength: 100,
        });
        if (!name) return;
        store.engine.create_style(kind, name);
        store.poll();
      },
    });
    menu.open(anchor, entries, { align: 'end', label: 'Styles' });
  };
  return { open, element: menu.element };
}

/** Section-header button that opens the style picker. */
export function StylePickerButton({ store, kind }: { store: EditorStore; kind: StyleKind }) {
  const ui = useUi(store);
  const picker = useStylePicker(store);
  if (!ui.canEdit) return null;
  return (
    <>
      <IconButton active={false} title={kind === 'text' ? 'Text styles' : 'Color styles'} onClick={(e: ReactMouseEvent<HTMLButtonElement>) => picker.open(e.currentTarget, kind)}>
        <SwatchBook size={14} />
      </IconButton>
      {picker.element}
    </>
  );
}

/** Shown instead of the raw values when the selection is linked to a style. */
export function StyleChip({ store, styleId, kind }: { store: EditorStore; styleId: string; kind: StyleKind }) {
  const styles = useStyles(store);
  const ui = useUi(store);
  const [editing, setEditing] = useState(false);
  const style = styles.find((s) => s.id === styleId);
  const detach = () => {
    store.engine.detach_style(kind);
    store.poll();
  };
  if (!style) {
    return (
      <div className="style-chip missing">
        <span className="style-chip-name">Missing style</span>
        {ui.canEdit && (
          <IconButton active={false} title="Detach" onClick={detach}>
            <Unlink size={13} />
          </IconButton>
        )}
      </div>
    );
  }
  return (
    <div className="style-chip">
      <StyleSwatch store={store} style={style} />
      <span className="style-chip-name" title={style.description || style.name}>
        {style.name}
        {style.type === 'TEXT' && <small>{textStyleSummary(style)}</small>}
      </span>
      {ui.canEdit && (
        <>
          <IconButton active={false} title="Edit style" onClick={() => setEditing(true)}>
            <Pencil size={13} />
          </IconButton>
          <IconButton active={false} title="Detach style (keep the current look)" onClick={detach}>
            <Unlink size={13} />
          </IconButton>
        </>
      )}
      {editing && <StyleEditor store={store} style={style} onClose={() => setEditing(false)} />}
    </div>
  );
}

/** Edit a style's name and values; every layer using it follows. */
export function StyleEditor({ store, style, onClose }: { store: EditorStore; style: StyleInfo; onClose: () => void }) {
  const dialogs = useDialogs();
  const styles = useStyles(store);
  const live = styles.find((s) => s.id === style.id) ?? style;
  const [name, setName] = useState(live.name);
  const [description, setDescription] = useState(live.description);
  useEffect(() => {
    if (!styles.some((s) => s.id === style.id)) onClose();
  }, [styles, style.id, onClose]);
  const update = (props: Record<string, unknown>) => {
    store.engine.update_style(style.id, JSON.stringify(props));
    store.poll();
  };
  const begin = () => store.engine.begin('edit style');
  const end = () => {
    store.engine.end();
    store.poll();
  };
  const remove = async () => {
    const ok = await dialogs.confirm({
      title: `Delete “${live.name}”?`,
      message: `${live.uses} layer${live.uses === 1 ? '' : 's'} using it will keep their current look but won't follow the style anymore.`,
      confirmLabel: 'Delete style',
      danger: true,
    });
    if (!ok) return;
    store.engine.delete_style(style.id);
    store.poll();
    onClose();
  };
  const solid = live.fills.length === 1 && live.fills[0].type === 'SOLID' ? live.fills[0] : null;

  return (
    <Modal
      open
      onClose={onClose}
      title={live.type === 'TEXT' ? 'Text style' : 'Color style'}
      description={`Used by ${live.uses} layer${live.uses === 1 ? '' : 's'}. Changes apply to all of them, for everyone.`}
      footer={
        <>
          <button type="button" className="btn btn-ghost btn-danger-text" onClick={() => void remove()}>
            <Trash size={14} /> Delete
          </button>
          <span style={{ flex: 1 }} />
          <button type="button" className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </>
      }
    >
      <div className="style-editor">
        <label className="field">
          <span className="field-label">Name</span>
          <input
            className="input"
            value={name}
            maxLength={100}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() && name.trim() !== live.name && update({ name: name.trim() })}
          />
        </label>
        {live.type === 'FILL' ? (
          <div className="field">
            <span className="field-label">Color</span>
            {solid ? (
              <ColorInput value={solid.color} onChange={(hex) => update({ fills: [{ ...solid, color: hex, opacity: 1 }] })} onPickStart={begin} onPickEnd={end} />
            ) : (
              <div className="style-editor-paints">
                <span className="style-swatch big" style={{ background: paintsCss(live.fills, (id) => store.assetUrl(id)) }} />
                <span className="text-muted">Gradient and image styles: change a layer's fill, then use “Create color style” with the same name to replace it.</span>
              </div>
            )}
          </div>
        ) : (
          <div className="field">
            <span className="field-label">Typography</span>
            <FontPicker store={store} value={live.fontFamily} styles={[[live.fontWeight, live.italic]]} onPick={(f) => update({ fontFamily: f })} />
            <div className="grid2" style={{ marginTop: 6 }}>
              <Select
                value={String(live.fontWeight)}
                options={WEIGHTS.map((w) => ({ value: String(w), label: weightName(w) }))}
                onChange={(v) => update({ fontWeight: Number(v) })}
                title="Weight"
              />
              <NumberInput label="Size" value={live.fontSize} min={1} max={2000} onChange={(v) => update({ fontSize: v })} onScrubStart={begin} onScrubEnd={end} />
              <NumberInput
                label="LH"
                title="Line height (px). Clear for auto."
                value={live.lineHeight ?? Math.round(live.fontSize * 1.2 * 100) / 100}
                min={0}
                onChange={(v) => update({ lineHeight: v })}
                onScrubStart={begin}
                onScrubEnd={end}
              />
              <NumberInput label="LS" title="Letter spacing (px)" value={live.letterSpacing} step={0.1} onChange={(v) => update({ letterSpacing: v })} onScrubStart={begin} onScrubEnd={end} />
            </div>
          </div>
        )}
        <label className="field">
          <span className="field-label">Description</span>
          <input
            className="input"
            value={description}
            placeholder="When to use it (shown to collaborators and agents)"
            onChange={(e) => setDescription(e.target.value)}
            onBlur={() => description !== live.description && update({ description })}
          />
        </label>
      </div>
    </Modal>
  );
}
