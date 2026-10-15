// Right-hand "Design" panel. Reads the selection snapshot from the store and
// writes canonical node props back through the engine (one undo step per edit;
// scrubs and color drags are wrapped in begin/end).
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  AlertTriangle,
  AlignCenterHorizontal,
  AlignCenterVertical,
  AlignEndHorizontal,
  AlignEndVertical,
  AlignStartHorizontal,
  AlignStartVertical,
  ArrowDown,
  ArrowRight,
  LayoutGrid,
  Eye,
  EyeOff,
  FlipHorizontal2,
  FlipVertical2,
  ImageIcon,
  Loader2,
  Minus,
  Plus,
  RotateCw,
  Scan,
  Spline,
} from 'lucide-react';
import { api, type GoogleFontFamily } from '../lib/api';
import { familyWeights, findGoogleFamily, googleCatalogue, WEIGHTS, weightName } from '../lib/fonts';
import { FontPicker } from './FontPicker';
import { StyleChip, StylePickerButton } from './Styles';
import { useToast } from '../components/Toast';
import { useFontState, usePages, useSelection, useUi, type EditorStore } from './store';
import type { ConstraintType, Constraints, EffectValue, FillMethod, FillOrigin, ImageFilled, NineSlice, NodeData, Paint } from './types';
import { FILL_ORIGINS } from './types';
import { ColorInput, IconButton, NumberInput, Section, Select, splitAlpha, withAlpha } from './inputs';
import { BrushPanel } from './BrushPanel';

type Props = { store: EditorStore };

function common<T>(nodes: NodeData[], get: (n: NodeData) => T): T | 'mixed' {
  if (!nodes.length) return 'mixed';
  const first = JSON.stringify(get(nodes[0]));
  for (const n of nodes) if (JSON.stringify(get(n)) !== first) return 'mixed';
  return get(nodes[0]);
}



export function PropertiesPanel({ store }: Props) {
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
  if ((ui.tool === 'brush' || ui.tool === 'eraser') && !readOnly) return <BrushPanel store={store} />;
  if (!sel.length) return <PagePanel store={store} />;

  const types = new Set(sel.map((n) => n.type));
  const only = (t: string) => types.size === 1 && types.has(t as NodeData['type']);
  const n0 = sel[0];
  const isFrame = only('FRAME');
  const isText = only('TEXT');
  // Paint layers have pixels, not fills, strokes or effects.
  const isPaint = only('PAINT');
  const anyPaint = types.has('PAINT');
  const hasCorners = sel.every((n) => n.type === 'RECTANGLE' || n.type === 'FRAME');
  const inAutoLayout = sel.every((n) => n.parentLayout !== 'NONE' && !n.layoutAbsolute);

  return (
    <div className={`props${readOnly ? ' readonly' : ''}`}>
      <ComponentBanner store={store} sel={sel} readOnly={readOnly} />
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
        {isFrame && common(sel, (n) => n.clipsContent) === true && (
          <div className="row-actions">
            <label className="check" title="Clip children to the fill's alpha, so transparent image pixels hide them">
              <input type="checkbox" checked={common(sel, (n) => n.clipAlpha) === true} onChange={(e) => set({ clipAlpha: e.target.checked })} disabled={readOnly} />
              Alpha mask
            </label>
            {common(sel, (n) => n.clipAlpha) === true && (
              <label className="check" title="Don't draw the fill; it still masks the children">
                <input type="checkbox" checked={common(sel, (n) => n.hideFill) === true} onChange={(e) => set({ hideFill: e.target.checked })} disabled={readOnly} />
                Hide fill
              </label>
            )}
          </div>
        )}
      </Section>

      {isFrame && <AutoLayoutSection sel={sel} set={set} begin={begin} end={end} store={store} />}

      {sel.every(constrained) && <ConstraintsSection sel={sel} store={store} readOnly={readOnly} />}

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

      {isText && <TextSection sel={sel} set={set} begin={begin} end={end} store={store} readOnly={readOnly} />}

      {isPaint && (
        <Section title="Painting">
          <label className="check" title="Paint only where the layer already has pixels">
            <input type="checkbox" checked={common(sel, (n) => n.alphaLock) === true} onChange={(e) => set({ alphaLock: e.target.checked })} disabled={readOnly} />
            Lock transparent pixels
          </label>
        </Section>
      )}
      {!anyPaint && (
        <>
          <PaintsSection title="Fill" kind="fills" sel={sel} set={set} begin={begin} end={end} store={store} />
          <PaintsSection title="Stroke" kind="strokes" sel={sel} set={set} begin={begin} end={end} store={store} />
          <EffectsSection sel={sel} set={set} begin={begin} end={end} />
        </>
      )}
      <ExportSection store={store} sel={sel} />
      {sel.length === 1 && <NotesSection node={n0} set={set} readOnly={readOnly} />}
      {sel.length === 1 && (n0.key || (n0.meta && Object.keys(n0.meta).length > 0)) && <MetaSection node={n0} />}
    </div>
  );
}

function typeLabel(t: NodeData['type']) {
  if (t === 'PAINT') return 'Paint layer';
  return t.charAt(0) + t.slice(1).toLowerCase();
}

/** Component / instance context at the top of the Design panel. */
function ComponentBanner({ store, sel, readOnly }: { store: EditorStore; sel: NodeData[]; readOnly: boolean }) {
  const e = store.engine;
  const act = (f: () => void) => () => {
    f();
    store.poll();
  };
  if (sel.length !== 1) {
    const anyInstance = sel.some((n) => n.type === 'INSTANCE' && !n.derived);
    if (!anyInstance || readOnly) return null;
    return (
      <div className="component-banner">
        <div className="row-actions">
          <button type="button" className="btn btn-xs" onClick={act(() => e.detach_instances())}>
            Detach instances
          </button>
        </div>
      </div>
    );
  }
  const n = sel[0];
  if (n.type === 'COMPONENT') {
    return (
      <div className="component-banner">
        <span>
          <b>❖ Main component</b> · {n.instanceCount ?? 0} instance{n.instanceCount === 1 ? '' : 's'}
        </span>
        {!readOnly && (
          <div className="row-actions">
            <button type="button" className="btn btn-xs" onClick={act(() => e.insert_instance(n.id))}>
              Create instance
            </button>
          </div>
        )}
      </div>
    );
  }
  if (n.type === 'INSTANCE' && !n.derived) {
    return (
      <div className="component-banner">
        <span>
          <b>◇ {n.componentName ?? 'Missing component'}</b> instance
          {n.overrideCount ? ` · ${n.overrideCount} override${n.overrideCount === 1 ? '' : 's'}` : ''}
        </span>
        <div className="row-actions">
          <button type="button" className="btn btn-xs" onClick={act(() => e.go_to_main_component(n.id))}>
            Go to main
          </button>
          {!readOnly && (
            <>
              {!!n.overrideCount && (
                <button type="button" className="btn btn-xs" onClick={act(() => e.reset_overrides(JSON.stringify([n.id])))}>
                  Reset
                </button>
              )}
              <button type="button" className="btn btn-xs" onClick={act(() => e.detach_instances())}>
                Detach
              </button>
            </>
          )}
        </div>
      </div>
    );
  }
  if (n.derived) {
    return (
      <div className="component-banner">
        <span>
          <b>Inside an instance</b> — changes here are saved as overrides; the rest follows the main component.
        </span>
      </div>
    );
  }
  if (!readOnly && (n.type === 'FRAME' || n.type === 'GROUP')) {
    return (
      <div className="component-banner subtle">
        <div className="row-actions">
          <button type="button" className="btn btn-xs" onClick={act(() => e.create_component())} title="⌥⌘K">
            ❖ Create component
          </button>
        </div>
      </div>
    );
  }
  return null;
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
  // A grid aligns its block of cells like a horizontal layout: main = x.
  const horizontal = mode === 'HORIZONTAL' || mode === 'GRID';
  const grid = mode === 'GRID';
  const wrap = common(sel, (n) => n.layoutWrap) === true;
  const cell = common(sel, (n) => n.gridCellSize ?? null);
  const c = cell === 'mixed' ? null : cell;
  const setCell = (i: 0 | 1, v: number) => {
    const next: [number, number] = c ? [...c] : [0, 0];
    next[i] = Math.max(0, v);
    set({ gridCellSize: next[0] > 0 && next[1] > 0 ? next : null });
  };
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
            <button type="button" className={grid ? 'on' : ''} title="Grid" onClick={() => set({ layoutMode: 'GRID' })}>
              <LayoutGrid size={13} />
            </button>
          </div>
          <NumberInput label="Gap" title={grid ? 'Space between columns' : 'Spacing between items'} value={common(sel, (n) => n.itemSpacing)} min={0} onChange={(v) => set({ itemSpacing: v })} onScrubStart={begin} onScrubEnd={end} />
          {(grid || wrap) && (
            <NumberInput
              label={grid ? 'Rows' : 'Lines'}
              title={grid ? 'Space between rows' : 'Space between wrapped lines'}
              value={common(sel, (n) => n.counterAxisSpacing ?? n.itemSpacing)}
              min={0}
              onChange={(v) => set({ counterAxisSpacing: v })}
              onScrubStart={begin}
              onScrubEnd={end}
            />
          )}
          {!grid && (
            <label className="check">
              <input type="checkbox" checked={primary === 'SPACE_BETWEEN'} onChange={(e) => set({ primaryAxisAlign: e.target.checked ? 'SPACE_BETWEEN' : 'START' })} />
              Space between
            </label>
          )}
          {!grid && (
            <label className="check" title="Wrap items onto new lines (needs a fixed width or height)">
              <input type="checkbox" checked={wrap} onChange={(e) => set({ layoutWrap: e.target.checked })} />
              Wrap
            </label>
          )}
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
      {grid && (
        <div className="grid4">
          <NumberInput
            label="Cols"
            title="Columns (0 = as many as fit)"
            value={common(sel, (n) => n.gridColumns ?? 0)}
            min={0}
            precision={0}
            onChange={(v) => set({ gridColumns: v >= 1 ? Math.round(v) : null })}
          />
          <NumberInput label="W" title="Cell width (0 = the widest child)" value={c ? c[0] : 0} min={0} onChange={(v) => setCell(0, v)} onScrubStart={begin} onScrubEnd={end} />
          <NumberInput label="H" title="Cell height (0 = the tallest child)" value={c ? c[1] : 0} min={0} onChange={(v) => setCell(1, v)} onScrubStart={begin} onScrubEnd={end} />
        </div>
      )}
    </Section>
  );
}

const FRAME_LIKE: NodeData['type'][] = ['FRAME', 'COMPONENT', 'INSTANCE'];

/** Does the parent move this layer by its constraints (in a frame, not placed by auto layout)? */
function constrained(n: NodeData) {
  return n.parentType !== null && FRAME_LIKE.includes(n.parentType) && (n.parentLayout === 'NONE' || n.layoutAbsolute);
}

const H_OPTIONS: { value: ConstraintType; label: string }[] = [
  { value: 'MIN', label: 'Left' },
  { value: 'MAX', label: 'Right' },
  { value: 'STRETCH', label: 'Left & right' },
  { value: 'CENTER', label: 'Center' },
  { value: 'SCALE', label: 'Scale' },
];
const V_OPTIONS: { value: ConstraintType; label: string }[] = [
  { value: 'MIN', label: 'Top' },
  { value: 'MAX', label: 'Bottom' },
  { value: 'STRETCH', label: 'Top & bottom' },
  { value: 'CENTER', label: 'Center' },
  { value: 'SCALE', label: 'Scale' },
];

/** Clicking a side pins it; clicking the other side as well stretches, and clicking a stretched side unpins it. */
function togglePin(cur: ConstraintType | 'mixed', side: 'MIN' | 'MAX'): ConstraintType {
  const other = side === 'MIN' ? 'MAX' : 'MIN';
  if (cur === other) return 'STRETCH';
  if (cur === 'STRETCH') return other;
  return side;
}

function ConstraintsSection({ sel, store, readOnly }: { sel: NodeData[]; store: EditorStore; readOnly: boolean }) {
  const h = common(sel, (n) => n.constraints.horizontal);
  const v = common(sel, (n) => n.constraints.vertical);
  // Per layer, so a mixed selection keeps each layer's other axis.
  const put = (axis: keyof Constraints, value: ConstraintType) => {
    if (readOnly) return;
    store.engine.update_nodes(JSON.stringify(sel.map((n) => ({ id: n.id, props: { constraints: { ...n.constraints, [axis]: value } } }))));
    store.poll();
  };
  const on = (axis: ConstraintType | 'mixed', ...values: ConstraintType[]) => (values.includes(axis as ConstraintType) ? 'on' : '');
  return (
    <Section title="Constraints">
      <div className="al-grid constraints-grid">
        <div className="stack-tight">
          <Select title="Horizontal constraint" value={h} options={H_OPTIONS} onChange={(c) => put('horizontal', c)} />
          <Select title="Vertical constraint" value={v} options={V_OPTIONS} onChange={(c) => put('vertical', c)} />
        </div>
        <svg className={`constraint-box${readOnly ? ' readonly' : ''}`} viewBox="0 0 76 76" role="group" aria-label="Constraints">
          <rect className="frame" x="6" y="6" width="64" height="64" rx="3" />
          <rect className="layer" x="27" y="27" width="22" height="22" rx="2" />
          <line className={on(h, 'MIN', 'STRETCH')} x1="10" y1="38" x2="23" y2="38" />
          <line className={on(h, 'MAX', 'STRETCH')} x1="53" y1="38" x2="66" y2="38" />
          <line className={on(v, 'MIN', 'STRETCH')} x1="38" y1="10" x2="38" y2="23" />
          <line className={on(v, 'MAX', 'STRETCH')} x1="38" y1="53" x2="38" y2="66" />
          <line className={on(h, 'CENTER')} x1="38" y1="31" x2="38" y2="45" />
          <line className={on(v, 'CENTER')} x1="31" y1="38" x2="45" y2="38" />
          <rect className="hit" x="6" y="30" width="20" height="16" onClick={() => put('horizontal', togglePin(h, 'MIN'))}><title>Left</title></rect>
          <rect className="hit" x="50" y="30" width="20" height="16" onClick={() => put('horizontal', togglePin(h, 'MAX'))}><title>Right</title></rect>
          <rect className="hit" x="30" y="6" width="16" height="20" onClick={() => put('vertical', togglePin(v, 'MIN'))}><title>Top</title></rect>
          <rect className="hit" x="30" y="50" width="16" height="20" onClick={() => put('vertical', togglePin(v, 'MAX'))}><title>Bottom</title></rect>
          <rect className="hit" x="34" y="27" width="8" height="22" onClick={() => put('horizontal', 'CENTER')}><title>Center horizontally</title></rect>
          <rect className="hit" x="27" y="34" width="22" height="8" onClick={() => put('vertical', 'CENTER')}><title>Center vertically</title></rect>
        </svg>
      </div>
    </Section>
  );
}

function TextSection({ sel, set, begin, end, store, readOnly }: SectionProps & { store: EditorStore; readOnly: boolean }) {
  useFontState(store); // re-render as fonts load
  const family = common(sel, (n) => n.fontFamily);
  const lh = common(sel, (n) => n.lineHeight);
  const deco = common(sel, (n) => n.textDecoration);
  const weight = common(sel, (n) => String(n.fontWeight));
  const styles = sel.map((n) => [n.fontWeight, n.italic] as [number, boolean]);
  const faces = family === 'mixed' ? undefined : store.fonts.family(family);
  const offered = faces ? familyWeights(faces) : WEIGHTS;
  const weights = weight !== 'mixed' && !offered.includes(Number(weight)) ? [...offered, Number(weight)].sort((a, b) => a - b) : offered;
  const missing = family !== 'mixed' && !store.fonts.isAvailable(family);

  // Load the face before applying a style, so text is measured with it.
  const applyStyle = async (props: { fontWeight?: number; italic?: boolean }) => {
    if (family !== 'mixed') {
      await store.fonts.prepare(family, sel.map((n) => [props.fontWeight ?? n.fontWeight, props.italic ?? n.italic]));
    }
    set(props);
  };

  const textStyleId = common(sel, (n) => n.textStyleId ?? null);
  return (
    <Section title="Text" actions={<StylePickerButton store={store} kind="text" />}>
      {typeof textStyleId === 'string' && textStyleId !== 'mixed' && <StyleChip store={store} styleId={textStyleId} kind="text" />}
      <FontPicker store={store} value={family === 'mixed' ? '' : family} styles={styles} disabled={readOnly} onPick={(f) => set({ fontFamily: f })} />
      {missing && <MissingFontNote store={store} family={family} />}
      <div className="grid2">
        <Select
          value={weight}
          options={weights.map((w) => ({ value: String(w), label: weightName(w) }))}
          onChange={(v) => void applyStyle({ fontWeight: Number(v) })}
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
          <button type="button" className={common(sel, (n) => n.italic) === true ? 'on' : ''} onClick={() => void applyStyle({ italic: !(common(sel, (n) => n.italic) === true) })} title="Italic">
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
      {common(sel, (n) => n.textAutoResize) !== 'WIDTH_AND_HEIGHT' && (
        <div className="grid2">
          <Select
            value={common(sel, (n) => n.textOverflow ?? 'VISIBLE')}
            title="Overflow: what happens to text that doesn't fit the box"
            options={[
              { value: 'VISIBLE', label: 'Overflow' },
              { value: 'CLIP', label: 'Clip' },
              { value: 'ELLIPSIS', label: 'Ellipsis…' },
              { value: 'TRUNCATE', label: 'Truncate' },
            ]}
            onChange={(v) => set({ textOverflow: v })}
          />
          <label className="check" title="Wrap lines at the box width">
            <input type="checkbox" checked={common(sel, (n) => n.textWrap ?? true) !== false} onChange={(e) => set({ textWrap: e.target.checked })} disabled={readOnly} />
            Wrap
          </label>
        </div>
      )}
      {common(sel, (n) => n.textAutoResize) === 'NONE' && <AutoFontSize sel={sel} set={set} begin={begin} end={end} readOnly={readOnly} />}
    </Section>
  );
}

/** Auto size: a fixed text box shrinks its font within a range to fit (TextMeshPro's Auto Size). */
function AutoFontSize({ sel, set, begin, end, readOnly }: SectionProps & { readOnly: boolean }) {
  const range = common(sel, (n) => n.autoFontSize ?? null);
  const r = range === 'mixed' ? null : range;
  const size = common(sel, (n) => n.fontSize);
  const base = typeof size === 'number' ? size : 16;
  return (
    <>
      <label className="check" title="Shrink the font to fit the box">
        <input
          type="checkbox"
          checked={!!r}
          onChange={(e) => set({ autoFontSize: e.target.checked ? { min: Math.max(1, Math.round(base / 2)), max: base } : null })}
          disabled={readOnly}
        />
        Auto size
      </label>
      {r && (
        <div className="grid2">
          <NumberInput label="Min" title="Smallest font size" value={r.min} min={1} onChange={(v) => set({ autoFontSize: { ...r, min: Math.min(v, r.max) } })} onScrubStart={begin} onScrubEnd={end} disabled={readOnly} />
          <NumberInput label="Max" title="Largest font size" value={r.max} min={1} onChange={(v) => set({ autoFontSize: { ...r, max: Math.max(v, r.min) } })} onScrubStart={begin} onScrubEnd={end} disabled={readOnly} />
        </div>
      )}
    </>
  );
}

function defaultPaint(kind: 'fills' | 'strokes'): Paint {
  return { type: 'SOLID', color: kind === 'fills' ? '#d9d9d9' : '#000000' };
}

function PaintsSection({ title, kind, sel, set, begin, end, store }: SectionProps & { title: string; kind: 'fills' | 'strokes'; store: EditorStore }) {
  const toast = useToast();
  const fileRef = useRef<HTMLInputElement>(null);
  const replaceIndex = useRef(0);
  const paints = common(sel, (n) => n[kind]);
  const update = (next: Paint[]) => set({ [kind]: next });
  const stroke = kind === 'strokes';
  const styleKind = stroke ? 'stroke' : 'fill';
  const styleId = common(sel, (n) => (stroke ? n.strokeStyleId : n.fillStyleId) ?? null);
  const isTextOnly = sel.every((n) => n.type === 'TEXT');

  const upload = async (file: File, i: number, list: Paint[]) => {
    try {
      const a = await api.files.uploadAsset(store.fileId, file);
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
        <>
          <StylePickerButton store={store} kind={styleKind} />
          <IconButton active={false} title={`Add ${title.toLowerCase()}`} onClick={() => update([...(paints === 'mixed' ? [] : paints), defaultPaint(kind)])}>
            <Plus size={14} />
          </IconButton>
        </>
      }
    >
      {typeof styleId === 'string' && styleId !== 'mixed' ? (
        <StyleChip store={store} styleId={styleId} kind={styleKind} />
      ) : paints === 'mixed' ? (
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
                <span className="image-thumb" style={{ backgroundImage: `url(${store.assetUrl(p.asset)})` }} />
                {p.slice ? (
                  <span className="image-mode" title="9-slice ignores the fit mode">9-slice</span>
                ) : (
                  <Select
                    value={p.fit ?? 'FILL'}
                    options={[
                      { value: 'FILL', label: 'Fill' },
                      { value: 'FIT', label: 'Fit' },
                      { value: 'STRETCH', label: 'Stretch' },
                      { value: 'TILE', label: 'Tile' },
                    ]}
                    onChange={(v) => update(paints.map((q, j) => (j === i ? { ...p, fit: v } : q)))}
                  />
                )}
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
            {p.type === 'IMAGE' && !stroke && (
              <ImageOptions paint={p} onChange={(next) => update(paints.map((q, j) => (j === i ? next : q)))} store={store} begin={begin} end={end} />
            )}
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

type ImagePaint = Extract<Paint, { type: 'IMAGE' }>;

/** Tint (multiply color) and 9-slice settings of an image fill, like Unity's Image component. */
function ImageOptions({ paint, onChange, store, begin, end }: { paint: ImagePaint; onChange: (p: ImagePaint) => void; store: EditorStore; begin: () => void; end: () => void }) {
  const s = paint.slice ?? null;
  const setSlice = (next: Partial<NineSlice>) => s && onChange({ ...paint, slice: { ...s, ...next } });
  const toggleSlice = (on: boolean) => {
    if (!on) return onChange({ ...paint, slice: null });
    // Start with a quarter of the image's short side on every edge.
    const size = store.engine.image_size(paint.asset);
    const v = size ? Math.max(1, Math.round(Math.min(size[0], size[1]) / 4)) : 8;
    onChange({ ...paint, slice: { top: v, right: v, bottom: v, left: v } });
  };
  const inset = (key: 'top' | 'right' | 'bottom' | 'left', label: string, title: string) =>
    s && <NumberInput label={label} title={`${title} inset (image px)`} value={s[key]} min={0} precision={0} onChange={(v) => setSlice({ [key]: v })} onScrubStart={begin} onScrubEnd={end} />;
  return (
    <div className="image-options">
      <div className="image-tint">
        <span>Tint</span>
        <ColorInput
          value={paint.tint ?? '#ffffff'}
          onChange={(hex) => onChange({ ...paint, tint: hex.toLowerCase() === '#ffffff' ? null : hex })}
          onPickStart={begin}
          onPickEnd={end}
        />
      </div>
      {paint.fit === 'TILE' && !s && (
        <div className="grid2">
          <NumberInput
            label="Tile"
            title="Tile scale: layer px per image px"
            value={paint.tileScale ?? 1}
            min={0.01}
            step={0.05}
            precision={2}
            onChange={(v) => onChange({ ...paint, tileScale: v })}
            onScrubStart={begin}
            onScrubEnd={end}
          />
        </div>
      )}
      <label className="check">
        <input type="checkbox" checked={!!s} onChange={(e) => toggleSlice(e.target.checked)} />
        9-slice
      </label>
      {s && (
        <>
          <div className="grid4">
            {inset('top', 'T', 'Top')}
            {inset('right', 'R', 'Right')}
            {inset('bottom', 'B', 'Bottom')}
            {inset('left', 'L', 'Left')}
          </div>
          <div className="grid2">
            <NumberInput
              label="Scale"
              title="Layer px per image px for the border (0.5 for @2x art)"
              value={s.scale ?? 1}
              min={0.01}
              step={0.05}
              precision={2}
              onChange={(v) => setSlice({ scale: v })}
              onScrubStart={begin}
              onScrubEnd={end}
            />
            <label className="check">
              <input type="checkbox" checked={s.fillCenter !== false} onChange={(e) => setSlice({ fillCenter: e.target.checked })} />
              Fill center
            </label>
          </div>
        </>
      )}
      <FilledOptions paint={paint} onChange={onChange} begin={begin} end={end} />
    </div>
  );
}

const ORIGIN_LABELS: Record<FillOrigin, string> = {
  LEFT: 'Left',
  RIGHT: 'Right',
  BOTTOM: 'Bottom',
  TOP: 'Top',
  BOTTOM_LEFT: 'Bottom left',
  TOP_LEFT: 'Top left',
  TOP_RIGHT: 'Top right',
  BOTTOM_RIGHT: 'Bottom right',
};

/** Unity's Filled image type: method, amount, origin and direction. */
function FilledOptions({ paint, onChange, begin, end }: { paint: ImagePaint; onChange: (p: ImagePaint) => void; begin: () => void; end: () => void }) {
  const f = paint.filled ?? null;
  const setMethod = (m: FillMethod | 'NONE') => {
    if (m === 'NONE') return onChange({ ...paint, filled: null });
    const origins = FILL_ORIGINS[m];
    const origin = f && origins.includes(f.origin) ? f.origin : origins[m === 'RADIAL_360' ? 2 : 0];
    onChange({ ...paint, filled: { method: m, amount: f?.amount ?? 1, origin, clockwise: f?.clockwise ?? true } });
  };
  const setF = (next: Partial<ImageFilled>) => f && onChange({ ...paint, filled: { ...f, ...next } });
  return (
    <>
      <div className="grid2">
        <Select
          value={f?.method ?? 'NONE'}
          title="Filled: show part of the image (Unity's Filled image type)"
          options={[
            { value: 'NONE', label: 'Not filled' },
            { value: 'HORIZONTAL', label: 'Horizontal' },
            { value: 'VERTICAL', label: 'Vertical' },
            { value: 'RADIAL_90', label: 'Radial 90' },
            { value: 'RADIAL_180', label: 'Radial 180' },
            { value: 'RADIAL_360', label: 'Radial 360' },
          ]}
          onChange={(v) => setMethod(v as FillMethod | 'NONE')}
        />
        {f && (
          <NumberInput
            label="Amount"
            title="Fill amount"
            value={Math.round(f.amount * 100)}
            min={0}
            max={100}
            suffix="%"
            precision={0}
            onChange={(v) => setF({ amount: Math.min(1, Math.max(0, v / 100)) })}
            onScrubStart={begin}
            onScrubEnd={end}
          />
        )}
      </div>
      {f && (
        <div className="grid2">
          <Select
            value={f.origin}
            title="Fill origin"
            options={FILL_ORIGINS[f.method].map((o) => ({ value: o, label: ORIGIN_LABELS[o] }))}
            onChange={(v) => setF({ origin: v as FillOrigin })}
          />
          {f.method.startsWith('RADIAL') && (
            <label className="check">
              <input type="checkbox" checked={f.clockwise !== false} onChange={(e) => setF({ clockwise: e.target.checked })} />
              Clockwise
            </label>
          )}
        </div>
      )}
    </>
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

function ExportSection({ store, sel }: { store: EditorStore; sel: NodeData[] }) {
  const toast = useToast();
  const one = sel.length === 1 ? sel[0] : null;
  return (
    <Section title="Export">
      {/* Exporting is allowed in read-only mode too. */}
      <div className="row-actions wrap readonly-ok">
        {[1, 2, 3].map((s) => (
          <a key={s} className="btn btn-xs" href={one ? store.exportPngUrl({ node: one.id, scale: s }) : undefined} download={one ? `${safeName(one.name)}@${s}x.png` : undefined} aria-disabled={!one}>
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

/** Read-only: the key and meta that exports and agents attached. */
function MetaSection({ node }: { node: NodeData }) {
  const entries = Object.entries(node.meta ?? {});
  return (
    <Section title="Data">
      <dl className="prop-meta">
        {node.key && (
          <>
            <dt>key</dt>
            <dd title={node.key}>{node.key}</dd>
          </>
        )}
        {entries.map(([k, v]) => {
          const text = typeof v === 'string' ? v : JSON.stringify(v);
          return (
            <Fragment key={k}>
              <dt>{k}</dt>
              <dd title={text}>{text}</dd>
            </Fragment>
          );
        })}
      </dl>
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
  const { bg, linear } = useMemo(() => {
    const raw = page ? store.engine.node(page.id) : undefined;
    if (!raw) return { bg: '#f5f5f5', linear: false };
    const n = JSON.parse(raw) as { background: string | null; linearBlending?: boolean };
    return { bg: n.background ?? '#f5f5f5', linear: n.linearBlending === true };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, store, ui]);
  return (
    <div className={`props${ui.canEdit ? '' : ' readonly'}`}>
      <Section title="Page">
        <ColorInput
          value={bg}
          onChange={(hex) => {
            if (!page || !ui.canEdit) return;
            store.engine.update_nodes(JSON.stringify([{ id: page.id, props: { background: hex } }]));
            store.poll();
          }}
        />
        <label className="check" title="Blend semi-transparent layers in linear light, like a Unity project in Linear color space">
          <input
            type="checkbox"
            checked={linear}
            disabled={!ui.canEdit}
            onChange={(e) => {
              if (!page || !ui.canEdit) return;
              store.engine.update_nodes(JSON.stringify([{ id: page.id, props: { linearBlending: e.target.checked } }]));
              store.poll();
            }}
          />
          Linear blending (Unity)
        </label>
      </Section>
      <Section title="Tips">
        <ul className="tips">
          {ui.canEdit ? (
            <>
              <Tip k="F">Frame</Tip>
              <Tip k="R / O / L">Rectangle, ellipse, line</Tip>
              <Tip k="T">Text</Tip>
              <Tip k="P">Pen</Tip>
              <Tip k="Shift A">Auto layout</Tip>
              <Tip k="⌘ G">Group</Tip>
            </>
          ) : (
            <>
              <Tip k="Click">Inspect a layer</Tip>
              <Tip k="Shift 2">Zoom to selection</Tip>
            </>
          )}
          <Tip k="Space-drag">Pan</Tip>
          <Tip k="⌘ scroll">Zoom</Tip>
          <Tip k="Shift 1">Zoom to fit</Tip>
        </ul>
      </Section>
    </div>
  );
}

/** Shown when the selected text's font can't be displayed here. */
function MissingFontNote({ store, family }: { store: EditorStore; family: string }) {
  const ui = useUi(store);
  const toast = useToast();
  const [google, setGoogle] = useState<GoogleFontFamily | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const canAdd = ui.canEdit && !!store.workspaceId;
  useEffect(() => {
    let live = true;
    googleCatalogue().then(
      (list) => live && setGoogle(findGoogleFamily(list, family) ?? null),
      () => live && setGoogle(null),
    );
    return () => {
      live = false;
    };
  }, [family]);
  const add = async () => {
    if (!google || !store.workspaceId) return;
    setBusy(true);
    try {
      await api.fonts.addGoogle(store.workspaceId, google.family);
      await store.fonts.refresh();
      toast.success(`Added ${google.family} to the workspace`, { id: 'font-added' });
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="font-missing" role="status">
      <AlertTriangle size={13} />
      <div>
        <b>{family}</b> isn't available here, so a fallback font is shown.
        {canAdd && google && (
          <button type="button" className="btn btn-xs" disabled={busy} onClick={() => void add()}>
            {busy && <Loader2 size={11} className="spin" />} Add from Google Fonts
          </button>
        )}
        {canAdd && google === null && <span className="font-missing-hint"> Upload it from the font menu, or pick another font.</span>}
        {!canAdd && <span className="font-missing-hint"> Ask an editor to add it to the workspace.</span>}
      </div>
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
