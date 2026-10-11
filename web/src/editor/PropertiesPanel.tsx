// Right-hand "Design" panel. Reads the selection snapshot from the store and
// writes canonical node props back through the engine (one undo step per edit;
// scrubs and color drags are wrapped in begin/end).
import { useMemo, useRef, type ReactNode } from 'react';
import {
  AlignCenterHorizontal,
  AlignCenterVertical,
  AlignEndHorizontal,
  AlignEndVertical,
  AlignStartHorizontal,
  AlignStartVertical,
  ArrowDown,
  ArrowRight,
  Eye,
  EyeOff,
  FlipHorizontal2,
  FlipVertical2,
  ImageIcon,
  Minus,
  Plus,
  RotateCw,
  Scan,
  Spline,
} from 'lucide-react';
import { api, exportPngUrl } from '../lib/api';
import { useToast } from '../components/Toast';
import { usePages, useSelection, useUi, type EditorStore } from './store';
import type { EffectValue, NodeData, Paint } from './types';
import { ColorInput, IconButton, NumberInput, Section, Select, splitAlpha, withAlpha } from './inputs';

type Props = { store: EditorStore; fileId: string };

function common<T>(nodes: NodeData[], get: (n: NodeData) => T): T | 'mixed' {
  if (!nodes.length) return 'mixed';
  const first = JSON.stringify(get(nodes[0]));
  for (const n of nodes) if (JSON.stringify(get(n)) !== first) return 'mixed';
  return get(nodes[0]);
}

const FONTS = [
  'Inter',
  'system-ui',
  'Arial',
  'Helvetica',
  'Georgia',
  'Times New Roman',
  'Courier New',
  'Menlo',
  'Verdana',
  'Trebuchet MS',
  'Tahoma',
  'Impact',
];

const WEIGHTS = [
  { value: '100', label: 'Thin' },
  { value: '200', label: 'Extra Light' },
  { value: '300', label: 'Light' },
  { value: '400', label: 'Regular' },
  { value: '500', label: 'Medium' },
  { value: '600', label: 'Semi Bold' },
  { value: '700', label: 'Bold' },
  { value: '800', label: 'Extra Bold' },
  { value: '900', label: 'Black' },
];

export function PropertiesPanel({ store, fileId }: Props) {
  const sel = useSelection(store);
  const ui = useUi(store);
  const engine = store.engine;
  const readOnly = !ui.canEdit;

  const set = (props: Record<string, unknown>) => {
    if (readOnly) return;
    engine.update_selection(JSON.stringify(props));
    store.poll();
  };
  const begin = () => engine.begin('edit');
  const end = () => {
    engine.end();
    store.poll();
  };

  if (ui.tool === 'frame' && !readOnly) return <FramePresets store={store} />;
  if (!sel.length) return <PagePanel store={store} />;

  const types = new Set(sel.map((n) => n.type));
  const only = (t: string) => types.size === 1 && types.has(t as NodeData['type']);
  const n0 = sel[0];
  const isFrame = only('FRAME');
  const isText = only('TEXT');
  const hasCorners = sel.every((n) => n.type === 'RECTANGLE' || n.type === 'FRAME');
  const inAutoLayout = sel.every((n) => n.parentLayout !== 'NONE' && !n.layoutAbsolute);

  return (
    <div className={`props${readOnly ? ' readonly' : ''}`}>
      {!readOnly && (
        <div className="align-row">
          <IconButton active={false} title="Align left" onClick={() => engine.align('left')}>
            <AlignStartVertical size={15} />
          </IconButton>
          <IconButton active={false} title="Align horizontal centers" onClick={() => engine.align('hcenter')}>
            <AlignCenterVertical size={15} />
          </IconButton>
          <IconButton active={false} title="Align right" onClick={() => engine.align('right')}>
            <AlignEndVertical size={15} />
          </IconButton>
          <IconButton active={false} title="Align top" onClick={() => engine.align('top')}>
            <AlignStartHorizontal size={15} />
          </IconButton>
          <IconButton active={false} title="Align vertical centers" onClick={() => engine.align('vcenter')}>
            <AlignCenterHorizontal size={15} />
          </IconButton>
          <IconButton active={false} title="Align bottom" onClick={() => engine.align('bottom')}>
            <AlignEndHorizontal size={15} />
          </IconButton>
          {sel.length >= 3 && (
            <>
              <IconButton active={false} title="Distribute horizontally" onClick={() => engine.distribute(true)}>
                <span className="glyph">⇹</span>
              </IconButton>
              <IconButton active={false} title="Distribute vertically" onClick={() => engine.distribute(false)}>
                <span className="glyph">⇳</span>
              </IconButton>
            </>
          )}
        </div>
      )}

      <Section title={sel.length > 1 ? `${sel.length} layers` : typeLabel(n0.type)}>
        <div className="grid2">
          <NumberInput label="X" value={common(sel, (n) => n.x)} onChange={(v) => set({ x: v })} onScrubStart={begin} onScrubEnd={end} disabled={readOnly || inAutoLayout} />
          <NumberInput label="Y" value={common(sel, (n) => n.y)} onChange={(v) => set({ y: v })} onScrubStart={begin} onScrubEnd={end} disabled={readOnly || inAutoLayout} />
          <NumberInput label="W" value={common(sel, (n) => n.width)} min={0} onChange={(v) => set({ width: v })} onScrubStart={begin} onScrubEnd={end} disabled={readOnly} />
          <NumberInput
            label="H"
            value={common(sel, (n) => n.height)}
            min={0}
            onChange={(v) => set({ height: v })}
            onScrubStart={begin}
            onScrubEnd={end}
            disabled={readOnly || only('LINE')}
          />
        </div>
        {(isFrame || inAutoLayout) && (
          <div className="grid2">
            <Select
              title="Horizontal resizing"
              value={common(sel, (n) => n.layoutSizingHorizontal)}
              options={sizingOptions(sel, inAutoLayout)}
              onChange={(v) => set({ layoutSizingHorizontal: v, ...(v === 'FILL' && isText ? { textAutoResize: 'HEIGHT' } : {}) })}
            />
            <Select
              title="Vertical resizing"
              value={common(sel, (n) => n.layoutSizingVertical)}
              options={sizingOptions(sel, inAutoLayout)}
              onChange={(v) => set({ layoutSizingVertical: v })}
            />
          </div>
        )}
        <div className="grid2">
          <NumberInput
            label={<RotateCw size={12} />}
            title="Rotation"
            value={common(sel, (n) => n.rotation)}
            suffix="°"
            onChange={(v) => set({ rotation: ((((v + 180) % 360) + 360) % 360) - 180 })}
            onScrubStart={begin}
            onScrubEnd={end}
            disabled={readOnly}
          />
          {hasCorners ? (
            <NumberInput
              label={<Scan size={12} />}
              title="Corner radius"
              value={common(sel, (n) => n.cornerRadius)}
              min={0}
              onChange={(v) => set({ cornerRadius: v, cornerRadii: null })}
              onScrubStart={begin}
              onScrubEnd={end}
              disabled={readOnly}
            />
          ) : only('POLYGON') || only('STAR') ? (
            <NumberInput label="#" title="Point count" value={common(sel, (n) => n.pointCount)} min={3} max={60} precision={0} onChange={(v) => set({ pointCount: Math.round(v) })} disabled={readOnly} />
          ) : (
            <span />
          )}
        </div>
        {only('STAR') && (
          <div className="grid2">
            <NumberInput label="Ratio" title="Inner radius" value={common(sel, (n) => Math.round(n.innerRadius * 100))} min={1} max={100} suffix="%" onChange={(v) => set({ innerRadius: v / 100 })} disabled={readOnly} />
          </div>
        )}
        <div className="row-actions">
          <IconButton active={false} title="Flip horizontal (Shift+H)" onClick={() => engine.flip(true)} disabled={readOnly}>
            <FlipHorizontal2 size={14} />
          </IconButton>
          <IconButton active={false} title="Flip vertical (Shift+V)" onClick={() => engine.flip(false)} disabled={readOnly}>
            <FlipVertical2 size={14} />
          </IconButton>
          {isFrame && (
            <label className="check">
              <input type="checkbox" checked={common(sel, (n) => n.clipsContent) === true} onChange={(e) => set({ clipsContent: e.target.checked })} disabled={readOnly} />
              Clip content
            </label>
          )}
        </div>
      </Section>

      {isFrame && <AutoLayoutSection sel={sel} set={set} begin={begin} end={end} store={store} />}

      <Section title="Layer">
        <div className="grid2">
          <NumberInput
            label="Opacity"
            value={common(sel, (n) => Math.round(n.opacity * 100))}
            min={0}
            max={100}
            precision={0}
            suffix="%"
            onChange={(v) => set({ opacity: v / 100 })}
            onScrubStart={begin}
            onScrubEnd={end}
            disabled={readOnly}
          />
          <IconButton
            active={common(sel, (n) => n.visible) === false}
            title="Toggle visibility (Cmd+Shift+H)"
            onClick={() => engine.toggle_visible(JSON.stringify(sel.map((n) => n.id)))}
            disabled={readOnly}
          >
            {common(sel, (n) => n.visible) === false ? <EyeOff size={14} /> : <Eye size={14} />}
          </IconButton>
        </div>
      </Section>

      {isText && <TextSection sel={sel} set={set} begin={begin} end={end} />}

      <PaintsSection title="Fill" kind="fills" sel={sel} set={set} begin={begin} end={end} fileId={fileId} />
      <PaintsSection title="Stroke" kind="strokes" sel={sel} set={set} begin={begin} end={end} fileId={fileId} />
      <EffectsSection sel={sel} set={set} begin={begin} end={end} />
      <ExportSection store={store} fileId={fileId} sel={sel} />
      {sel.length === 1 && <NotesSection node={n0} set={set} readOnly={readOnly} />}
    </div>
  );
}

function typeLabel(t: NodeData['type']) {
  return t.charAt(0) + t.slice(1).toLowerCase();
}

function sizingOptions(sel: NodeData[], inAutoLayout: boolean) {
  const opts: { value: NodeData['layoutSizingHorizontal']; label: string }[] = [{ value: 'FIXED', label: 'Fixed' }];
  if (sel.every((n) => n.type === 'FRAME' && n.layoutMode !== 'NONE') || sel.every((n) => n.type === 'TEXT')) opts.push({ value: 'HUG', label: 'Hug' });
  if (inAutoLayout) opts.push({ value: 'FILL', label: 'Fill' });
  return opts;
}

type SectionProps = {
  sel: NodeData[];
  set: (p: Record<string, unknown>) => void;
  begin: () => void;
  end: () => void;
};

function AutoLayoutSection({ sel, set, begin, end, store }: SectionProps & { store: EditorStore }) {
  const mode = common(sel, (n) => n.layoutMode);
  if (mode === 'NONE') {
    return (
      <Section
        title="Auto layout"
        actions={
          <IconButton active={false} title="Add auto layout (Shift+A)" onClick={() => store.engine.add_auto_layout()}>
            <Plus size={14} />
          </IconButton>
        }
      />
    );
  }
  const pad = common(sel, (n) => n.padding);
  const p = pad === 'mixed' ? null : pad;
  const primary = common(sel, (n) => n.primaryAxisAlign);
  const counter = common(sel, (n) => n.counterAxisAlign);
  const horizontal = mode === 'HORIZONTAL';
  const cells: ('START' | 'CENTER' | 'END')[] = ['START', 'CENTER', 'END'];
  return (
    <Section
      title="Auto layout"
      actions={
        <IconButton active={false} title="Remove auto layout" onClick={() => set({ layoutMode: 'NONE' })}>
          <Minus size={14} />
        </IconButton>
      }
    >
      <div className="al-grid">
        <div className="stack-tight">
          <div className="segmented-mini">
            <button type="button" className={mode === 'VERTICAL' ? 'on' : ''} title="Vertical" onClick={() => set({ layoutMode: 'VERTICAL' })}>
              <ArrowDown size={13} />
            </button>
            <button type="button" className={mode === 'HORIZONTAL' ? 'on' : ''} title="Horizontal" onClick={() => set({ layoutMode: 'HORIZONTAL' })}>
              <ArrowRight size={13} />
            </button>
          </div>
          <NumberInput label="Gap" title="Spacing between items" value={common(sel, (n) => n.itemSpacing)} min={0} onChange={(v) => set({ itemSpacing: v })} onScrubStart={begin} onScrubEnd={end} />
          <label className="check">
            <input type="checkbox" checked={primary === 'SPACE_BETWEEN'} onChange={(e) => set({ primaryAxisAlign: e.target.checked ? 'SPACE_BETWEEN' : 'START' })} />
            Space between
          </label>
        </div>
        <div className="align-box" title="Alignment">
          {cells.map((row) =>
            cells.map((col) => {
              const main = horizontal ? col : row;
              const cross = horizontal ? row : col;
              const on = (primary === main || (primary === 'SPACE_BETWEEN' && main === 'START')) && counter === cross;
              return (
                <button
                  key={`${row}${col}`}
                  type="button"
                  className={on ? 'on' : ''}
                  onClick={() => set({ primaryAxisAlign: primary === 'SPACE_BETWEEN' ? 'SPACE_BETWEEN' : main, counterAxisAlign: cross })}
                  aria-label={`Align ${row.toLowerCase()} ${col.toLowerCase()}`}
                >
                  <span />
                </button>
              );
            }),
          )}
        </div>
      </div>
      <div className="grid4">
        <NumberInput label="T" title="Padding top" value={p ? p.top : 'mixed'} min={0} onChange={(v) => p && set({ padding: { ...p, top: v } })} onScrubStart={begin} onScrubEnd={end} />
        <NumberInput label="R" title="Padding right" value={p ? p.right : 'mixed'} min={0} onChange={(v) => p && set({ padding: { ...p, right: v } })} onScrubStart={begin} onScrubEnd={end} />
        <NumberInput label="B" title="Padding bottom" value={p ? p.bottom : 'mixed'} min={0} onChange={(v) => p && set({ padding: { ...p, bottom: v } })} onScrubStart={begin} onScrubEnd={end} />
        <NumberInput label="L" title="Padding left" value={p ? p.left : 'mixed'} min={0} onChange={(v) => p && set({ padding: { ...p, left: v } })} onScrubStart={begin} onScrubEnd={end} />
      </div>
    </Section>
  );
}

function TextSection({ sel, set, begin, end }: SectionProps) {
  const family = common(sel, (n) => n.fontFamily);
  const lh = common(sel, (n) => n.lineHeight);
  const deco = common(sel, (n) => n.textDecoration);
  return (
    <Section title="Text">
      <input
        className="prop-text"
        list="flexma-fonts"
        value={family === 'mixed' ? '' : family}
        placeholder="Mixed"
        onChange={(e) => e.target.value && set({ fontFamily: e.target.value })}
      />
      <datalist id="flexma-fonts">
        {FONTS.map((f) => (
          <option key={f} value={f} />
        ))}
      </datalist>
      <div className="grid2">
        <Select
          value={common(sel, (n) => String(n.fontWeight))}
          options={WEIGHTS}
          onChange={(v) => set({ fontWeight: Number(v) })}
          title="Weight"
        />
        <NumberInput label="Size" value={common(sel, (n) => n.fontSize)} min={1} max={2000} onChange={(v) => set({ fontSize: v })} onScrubStart={begin} onScrubEnd={end} />
        <NumberInput
          label="LH"
          title="Line height (px). Clear for auto."
          value={lh === 'mixed' ? 'mixed' : (lh ?? Math.round(sel[0].fontSize * 1.2 * 100) / 100)}
          min={0}
          onChange={(v) => set({ lineHeight: v })}
          onScrubStart={begin}
          onScrubEnd={end}
        />
        <NumberInput label="LS" title="Letter spacing (px)" value={common(sel, (n) => n.letterSpacing)} step={0.1} onChange={(v) => set({ letterSpacing: v })} onScrubStart={begin} onScrubEnd={end} />
      </div>
      <div className="row-actions wrap">
        <div className="segmented-mini">
          {(['LEFT', 'CENTER', 'RIGHT'] as const).map((a) => (
            <button key={a} type="button" className={common(sel, (n) => n.textAlign) === a ? 'on' : ''} onClick={() => set({ textAlign: a })} title={`Align ${a.toLowerCase()}`}>
              {a === 'LEFT' ? '⟸' : a === 'CENTER' ? '≡' : '⟹'}
            </button>
          ))}
        </div>
        <div className="segmented-mini">
          {(['TOP', 'CENTER', 'BOTTOM'] as const).map((a) => (
            <button key={a} type="button" className={common(sel, (n) => n.textAlignVertical) === a ? 'on' : ''} onClick={() => set({ textAlignVertical: a })} title={`Vertical ${a.toLowerCase()}`}>
              {a === 'TOP' ? '⤒' : a === 'CENTER' ? '↕' : '⤓'}
            </button>
          ))}
        </div>
        <div className="segmented-mini">
          <button type="button" className={common(sel, (n) => n.italic) === true ? 'on' : ''} onClick={() => set({ italic: !(common(sel, (n) => n.italic) === true) })} title="Italic">
            <i>I</i>
          </button>
          <button type="button" className={deco === 'UNDERLINE' ? 'on' : ''} onClick={() => set({ textDecoration: deco === 'UNDERLINE' ? null : 'UNDERLINE' })} title="Underline">
            <u>U</u>
          </button>
          <button type="button" className={deco === 'STRIKETHROUGH' ? 'on' : ''} onClick={() => set({ textDecoration: deco === 'STRIKETHROUGH' ? null : 'STRIKETHROUGH' })} title="Strikethrough">
            <s>S</s>
          </button>
        </div>
      </div>
      <Select
        value={common(sel, (n) => n.textAutoResize)}
        title="Resizing"
        options={[
          { value: 'WIDTH_AND_HEIGHT', label: 'Auto width' },
          { value: 'HEIGHT', label: 'Auto height' },
          { value: 'NONE', label: 'Fixed size' },
        ]}
        onChange={(v) => set({ textAutoResize: v })}
      />
    </Section>
  );
}

function defaultPaint(kind: 'fills' | 'strokes'): Paint {
  return { type: 'SOLID', color: kind === 'fills' ? '#d9d9d9' : '#000000' };
}

function PaintsSection({ title, kind, sel, set, begin, end, fileId }: SectionProps & { title: string; kind: 'fills' | 'strokes'; fileId: string }) {
  const toast = useToast();
  const fileRef = useRef<HTMLInputElement>(null);
  const replaceIndex = useRef(0);
  const paints = common(sel, (n) => n[kind]);
  const update = (next: Paint[]) => set({ [kind]: next });
  const stroke = kind === 'strokes';
  const isTextOnly = sel.every((n) => n.type === 'TEXT');

  const upload = async (file: File, i: number, list: Paint[]) => {
    try {
      const a = await api.files.uploadAsset(fileId, file);
      const next = [...list];
      next[i] = { type: 'IMAGE', asset: a.id, fit: 'FILL' };
      update(next);
    } catch (err) {
      toast.error(err);
    }
  };

  return (
    <Section
      title={title}
      actions={
        <IconButton active={false} title={`Add ${title.toLowerCase()}`} onClick={() => update([...(paints === 'mixed' ? [] : paints), defaultPaint(kind)])}>
          <Plus size={14} />
        </IconButton>
      }
    >
      {paints === 'mixed' ? (
        <div className="muted-row">Mixed — click + to replace</div>
      ) : (
        paints.map((p, i) => (
          <div className="paint-row" key={i}>
            {p.type === 'SOLID' && (
              <ColorInput
                value={p.opacity !== undefined && p.opacity < 1 ? withAlpha(splitAlpha(p.color).rgb, splitAlpha(p.color).alpha * p.opacity) : p.color}
                onChange={(hex) => update(paints.map((q, j) => (j === i ? { ...p, color: hex, opacity: 1 } : q)))}
                onPickStart={begin}
                onPickEnd={end}
              />
            )}
            {p.type === 'LINEAR_GRADIENT' && (
              <div className="gradient-row">
                <span className="gradient-swatch" style={{ background: `linear-gradient(${(p.angle ?? 0) + 90}deg, ${p.stops.map((s) => `${s.color} ${s.position * 100}%`).join(', ')})` }} />
                {p.stops.slice(0, 2).map((s, k) => (
                  <ColorInput
                    key={k}
                    value={s.color}
                    onChange={(hex) =>
                      update(paints.map((q, j) => (j === i ? { ...p, stops: p.stops.map((t, m) => (m === k ? { ...t, color: hex } : t)) } : q)))
                    }
                    onPickStart={begin}
                    onPickEnd={end}
                  />
                ))}
                <NumberInput label="∠" title="Angle" value={p.angle ?? 0} suffix="°" onChange={(v) => update(paints.map((q, j) => (j === i ? { ...p, angle: v } : q)))} onScrubStart={begin} onScrubEnd={end} />
              </div>
            )}
            {p.type === 'IMAGE' && (
              <div className="image-row">
                <span className="image-thumb" style={{ backgroundImage: `url(${api.assets.url(p.asset)})` }} />
                <Select
                  value={p.fit ?? 'FILL'}
                  options={[
                    { value: 'FILL', label: 'Fill' },
                    { value: 'FIT', label: 'Fit' },
                    { value: 'STRETCH', label: 'Stretch' },
                  ]}
                  onChange={(v) => update(paints.map((q, j) => (j === i ? { ...p, fit: v } : q)))}
                />
                <button
                  type="button"
                  className="btn btn-xs"
                  onClick={() => {
                    replaceIndex.current = i;
                    fileRef.current?.click();
                  }}
                >
                  Replace
                </button>
              </div>
            )}
            <div className="paint-actions">
              {!stroke && !isTextOnly && (
                <select
                  className="paint-type"
                  value={p.type}
                  title="Paint type"
                  onChange={(e) => {
                    const t = e.target.value;
                    const color = p.type === 'SOLID' ? p.color : '#6d5dfc';
                    if (t === 'SOLID') update(paints.map((q, j) => (j === i ? { type: 'SOLID', color } : q)));
                    if (t === 'LINEAR_GRADIENT')
                      update(
                        paints.map((q, j) =>
                          j === i ? { type: 'LINEAR_GRADIENT', angle: 90, stops: [{ position: 0, color }, { position: 1, color: '#ffffff' }] } : q,
                        ),
                      );
                    if (t === 'IMAGE') {
                      replaceIndex.current = i;
                      fileRef.current?.click();
                    }
                  }}
                >
                  <option value="SOLID">Solid</option>
                  <option value="LINEAR_GRADIENT">Gradient</option>
                  <option value="IMAGE">Image</option>
                </select>
              )}
              <IconButton
                active={p.visible === false}
                title={p.visible === false ? 'Show' : 'Hide'}
                onClick={() => update(paints.map((q, j) => (j === i ? { ...p, visible: p.visible === false } : q)))}
              >
                {p.visible === false ? <EyeOff size={13} /> : <Eye size={13} />}
              </IconButton>
              <IconButton active={false} title="Remove" onClick={() => update(paints.filter((_, j) => j !== i))}>
                <Minus size={13} />
              </IconButton>
            </div>
          </div>
        ))
      )}
      {stroke && paints !== 'mixed' && paints.length > 0 && (
        <div className="grid2">
          <NumberInput label="W" title="Stroke weight" value={common(sel, (n) => n.strokeWeight)} min={0} onChange={(v) => set({ strokeWeight: v })} onScrubStart={begin} onScrubEnd={end} />
          <Select
            value={common(sel, (n) => n.strokeAlign)}
            options={[
              { value: 'INSIDE', label: 'Inside' },
              { value: 'CENTER', label: 'Center' },
              { value: 'OUTSIDE', label: 'Outside' },
            ]}
            onChange={(v) => set({ strokeAlign: v })}
          />
        </div>
      )}
      <input
        ref={fileRef}
        type="file"
        accept="image/png,image/jpeg,image/gif,image/webp"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = '';
          if (f && paints !== 'mixed') {
            const list = paints.length ? paints : [defaultPaint(kind)];
            void upload(f, Math.min(replaceIndex.current, list.length - 1), list);
          }
        }}
      />
      {!stroke && paints !== 'mixed' && paints.length === 0 && (
        <button type="button" className="link-btn" onClick={() => fileRef.current?.click()}>
          <ImageIcon size={12} /> Add image fill
        </button>
      )}
    </Section>
  );
}

function EffectsSection({ sel, set, begin, end }: SectionProps) {
  const effects = common(sel, (n) => n.effects);
  const update = (next: EffectValue[]) => set({ effects: next });
  return (
    <Section
      title="Effects"
      actions={
        <IconButton
          active={false}
          title="Add drop shadow"
          onClick={() => update([...(effects === 'mixed' ? [] : effects), { type: 'DROP_SHADOW', color: '#00000040', x: 0, y: 4, blur: 12, spread: 0 }])}
        >
          <Plus size={14} />
        </IconButton>
      }
    >
      {effects !== 'mixed' &&
        effects.map((e, i) => (
          <div className="effect-row" key={i}>
            <div className="effect-head">
              <select
                className="paint-type"
                value={e.type}
                onChange={(ev) =>
                  update(
                    effects.map((q, j) =>
                      j === i ? (ev.target.value === 'LAYER_BLUR' ? { type: 'LAYER_BLUR', radius: 8 } : { type: 'DROP_SHADOW', color: '#00000040', x: 0, y: 4, blur: 12, spread: 0 }) : q,
                    ),
                  )
                }
              >
                <option value="DROP_SHADOW">Drop shadow</option>
                <option value="LAYER_BLUR">Layer blur (export only)</option>
              </select>
              <IconButton active={e.visible === false} title="Toggle" onClick={() => update(effects.map((q, j) => (j === i ? { ...e, visible: e.visible === false } : q)))}>
                {e.visible === false ? <EyeOff size={13} /> : <Eye size={13} />}
              </IconButton>
              <IconButton active={false} title="Remove" onClick={() => update(effects.filter((_, j) => j !== i))}>
                <Minus size={13} />
              </IconButton>
            </div>
            {e.type === 'DROP_SHADOW' ? (
              <>
                <div className="grid4">
                  <NumberInput label="X" value={e.x} onChange={(v) => update(effects.map((q, j) => (j === i ? { ...e, x: v } : q)))} onScrubStart={begin} onScrubEnd={end} />
                  <NumberInput label="Y" value={e.y} onChange={(v) => update(effects.map((q, j) => (j === i ? { ...e, y: v } : q)))} onScrubStart={begin} onScrubEnd={end} />
                  <NumberInput label="B" title="Blur" min={0} value={e.blur} onChange={(v) => update(effects.map((q, j) => (j === i ? { ...e, blur: v } : q)))} onScrubStart={begin} onScrubEnd={end} />
                  <NumberInput label="S" title="Spread" value={e.spread} onChange={(v) => update(effects.map((q, j) => (j === i ? { ...e, spread: v } : q)))} onScrubStart={begin} onScrubEnd={end} />
                </div>
                <ColorInput value={e.color} onChange={(hex) => update(effects.map((q, j) => (j === i ? { ...e, color: hex } : q)))} onPickStart={begin} onPickEnd={end} />
              </>
            ) : (
              <NumberInput label="Blur" min={0} value={e.radius} onChange={(v) => update(effects.map((q, j) => (j === i ? { ...e, radius: v } : q)))} onScrubStart={begin} onScrubEnd={end} />
            )}
          </div>
        ))}
    </Section>
  );
}

function download(name: string, blob: Blob) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function safeName(s: string) {
  return s.replace(/[^\w\- ]+/g, '').trim() || 'export';
}

function ExportSection({ store, fileId, sel }: { store: EditorStore; fileId: string; sel: NodeData[] }) {
  const toast = useToast();
  const one = sel.length === 1 ? sel[0] : null;
  return (
    <Section title="Export">
      <div className="row-actions wrap">
        {[1, 2, 3].map((s) => (
          <a key={s} className="btn btn-xs" href={one ? exportPngUrl(fileId, { node: one.id, scale: s }) : undefined} download={one ? `${safeName(one.name)}@${s}x.png` : undefined} aria-disabled={!one}>
            PNG {s}x
          </a>
        ))}
        <button
          type="button"
          className="btn btn-xs"
          onClick={() => {
            const svg = store.engine.export_svg(JSON.stringify(sel.map((n) => n.id)));
            if (!svg) {
              toast.error('Nothing to export');
              return;
            }
            download(`${safeName(one?.name ?? 'selection')}.svg`, new Blob([svg], { type: 'image/svg+xml' }));
          }}
        >
          SVG
        </button>
        <button
          type="button"
          className="btn btn-xs"
          title="Copy as SVG"
          onClick={async () => {
            const svg = store.engine.export_svg(JSON.stringify(sel.map((n) => n.id)));
            if (svg) {
              await navigator.clipboard?.writeText(svg).catch(() => undefined);
              toast.success('SVG copied');
            }
          }}
        >
          <Spline size={12} /> Copy SVG
        </button>
      </div>
    </Section>
  );
}

function NotesSection({ node, set, readOnly }: { node: NodeData; set: (p: Record<string, unknown>) => void; readOnly: boolean }) {
  return (
    <Section title="Notes">
      <textarea
        key={node.id}
        className="prop-notes"
        defaultValue={node.description}
        placeholder="Intent, specs or instructions for collaborators and AI agents…"
        disabled={readOnly}
        onBlur={(e) => {
          if (e.target.value !== node.description) set({ description: e.target.value });
        }}
      />
    </Section>
  );
}

const PRESETS: { group: string; items: [string, number, number][] }[] = [
  {
    group: 'Phone',
    items: [
      ['iPhone 16', 393, 852],
      ['iPhone 16 Pro Max', 440, 956],
      ['iPhone SE', 375, 667],
      ['Android Compact', 412, 917],
    ],
  },
  {
    group: 'Tablet',
    items: [
      ['iPad mini', 744, 1133],
      ['iPad Pro 11"', 834, 1194],
      ['Android Tablet', 800, 1280],
    ],
  },
  {
    group: 'Desktop',
    items: [
      ['Desktop', 1440, 1024],
      ['MacBook Air', 1280, 832],
      ['Wide', 1920, 1080],
    ],
  },
  {
    group: 'Social & slides',
    items: [
      ['Instagram post', 1080, 1080],
      ['Instagram story', 1080, 1920],
      ['Slide 16:9', 1920, 1080],
      ['Open Graph image', 1200, 630],
    ],
  },
];

function FramePresets({ store }: { store: EditorStore }) {
  return (
    <div className="props">
      {PRESETS.map((g) => (
        <Section key={g.group} title={g.group}>
          <ul className="presets">
            {g.items.map(([name, w, h]) => (
              <li key={name}>
                <button
                  type="button"
                  onClick={() => {
                    store.engine.create_frame_preset(name, w, h);
                    store.poll();
                  }}
                >
                  <span>{name}</span>
                  <span className="text-muted">
                    {w}×{h}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </Section>
      ))}
    </div>
  );
}

function PagePanel({ store }: { store: EditorStore }) {
  const pages = usePages(store);
  const ui = useUi(store);
  const page = pages.pages.find((p) => p.id === pages.current);
  const bg = useMemo(() => {
    const raw = page ? store.engine.node(page.id) : undefined;
    if (!raw) return '#f5f5f5';
    const n = JSON.parse(raw) as { background: string | null };
    return n.background ?? '#f5f5f5';
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, store, ui]);
  return (
    <div className="props">
      <Section title="Page">
        <ColorInput
          value={bg}
          onChange={(hex) => {
            if (!page || !ui.canEdit) return;
            store.engine.update_nodes(JSON.stringify([{ id: page.id, props: { background: hex } }]));
            store.poll();
          }}
        />
      </Section>
      <Section title="Tips">
        <ul className="tips">
          <Tip k="F">Frame</Tip>
          <Tip k="R / O / L">Rectangle, ellipse, line</Tip>
          <Tip k="T">Text</Tip>
          <Tip k="P">Pen</Tip>
          <Tip k="Shift A">Auto layout</Tip>
          <Tip k="⌘ G">Group</Tip>
          <Tip k="Space-drag">Pan</Tip>
          <Tip k="⌘ scroll">Zoom</Tip>
          <Tip k="Shift 1">Zoom to fit</Tip>
        </ul>
      </Section>
    </div>
  );
}

function Tip({ k, children }: { k: string; children: ReactNode }) {
  return (
    <li>
      <span>{children}</span>
      <kbd className="kbd">{k}</kbd>
    </li>
  );
}
