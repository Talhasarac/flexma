// Design panel while the brush or eraser is active: brush settings, presets
// and the paint layer being painted on.
import { Pipette, Plus } from 'lucide-react';
import { useSelection, useUi, type EditorStore } from './store';
import { ColorInput, IconButton, NumberInput, Section } from './inputs';
import type { BrushSettings, NodeData } from './types';

const PRESETS: { name: string; brush: Partial<BrushSettings> }[] = [
  { name: 'Round', brush: { size: 12, hardness: 0.8, flow: 1, spacing: 0.1, pressureSize: true, pressureFlow: false, smoothing: 0.35 } },
  { name: 'Pencil', brush: { size: 4, hardness: 0.9, flow: 0.9, spacing: 0.08, pressureSize: true, pressureFlow: true, smoothing: 0.2 } },
  { name: 'Ink', brush: { size: 8, hardness: 1, flow: 1, spacing: 0.06, pressureSize: true, pressureFlow: false, smoothing: 0.55 } },
  { name: 'Marker', brush: { size: 24, hardness: 0.7, flow: 0.3, spacing: 0.08, pressureSize: false, pressureFlow: true, smoothing: 0.3 } },
  { name: 'Airbrush', brush: { size: 80, hardness: 0, flow: 0.06, spacing: 0.05, pressureSize: false, pressureFlow: true, smoothing: 0.2 } },
];

const ERASER_PRESETS: { name: string; brush: Partial<BrushSettings> }[] = [
  { name: 'Hard', brush: { size: 20, hardness: 0.95, flow: 1, spacing: 0.08, pressureSize: false, pressureFlow: false } },
  { name: 'Soft', brush: { size: 60, hardness: 0.1, flow: 0.4, spacing: 0.06, pressureSize: false, pressureFlow: true } },
];

const byte = (v: number) => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, '0');

function toHex(c: BrushSettings['color']) {
  const hex = `#${byte(c[0])}${byte(c[1])}${byte(c[2])}`;
  return c[3] >= 1 ? hex : hex + byte(c[3]);
}

function fromHex(hex: string): BrushSettings['color'] | null {
  const m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(hex);
  if (!m) return null;
  const n = parseInt(m[1], 16);
  const a = m[2] ? parseInt(m[2], 16) / 255 : 1;
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, a];
}

const close = (a: Partial<BrushSettings>, b: BrushSettings) =>
  (Object.keys(a) as (keyof BrushSettings)[]).every((k) => {
    const v = a[k];
    return typeof v === 'number' ? Math.abs(v - (b[k] as number)) < 1e-3 : v === b[k];
  });

/** The browser's screen color picker (Chromium only). */
type EyeDropperCtor = new () => { open: () => Promise<{ sRGBHex: string }> };
const eyeDropper = (window as unknown as { EyeDropper?: EyeDropperCtor }).EyeDropper;

export function BrushPanel({ store }: { store: EditorStore }) {
  const ui = useUi(store);
  const sel = useSelection(store);
  const eraser = ui.tool === 'eraser';
  const b = store.brush(eraser);
  const set = (patch: Partial<BrushSettings>) => store.setBrush(eraser, patch);
  const layer: NodeData | null = sel.length === 1 && sel[0].type === 'PAINT' && !sel[0].derived ? sel[0] : null;
  const presets = eraser ? ERASER_PRESETS : PRESETS;
  const pct = (v: number) => Math.round(v * 100);

  const setLayer = (props: Record<string, unknown>) => {
    store.engine.update_selection(JSON.stringify(props));
    store.poll();
  };

  return (
    <div className="props">
      <Section title={eraser ? 'Eraser' : 'Brush'}>
        <div className="brush-presets">
          {presets.map((p) => (
            <button key={p.name} type="button" className={`chip${close(p.brush, b) ? ' active' : ''}`} onClick={() => set(p.brush)}>
              {p.name}
            </button>
          ))}
        </div>
        {!eraser && (
          <div className="brush-color">
            <ColorInput
              value={toHex(b.color)}
              onChange={(hex) => {
                const c = fromHex(hex);
                if (c) set({ color: c });
              }}
            />
            {eyeDropper && (
              <IconButton
                active={false}
                title="Pick a color from the screen"
                onClick={() => {
                  new eyeDropper()
                    .open()
                    .then((r) => {
                      const c = fromHex(r.sRGBHex);
                      if (c) set({ color: [c[0], c[1], c[2], b.color[3]] });
                    })
                    .catch(() => {});
                }}
              >
                <Pipette size={14} />
              </IconButton>
            )}
          </div>
        )}
        <div className="grid2">
          <NumberInput label="Size" title="Diameter in layer pixels ([ and ] change it)" value={Math.round(b.size * 10) / 10} min={0.5} max={2000} suffix="px" onChange={(v) => set({ size: v })} />
          <NumberInput label="Hard" title="Edge hardness" value={pct(b.hardness)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ hardness: v / 100 })} />
          <NumberInput label="Flow" title="How much paint each dab lays down; overlapping dabs build up" value={pct(b.flow)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ flow: v / 100 })} />
          <NumberInput label="Gap" title="Spacing between dabs, as a share of the size" value={pct(b.spacing)} min={2} max={200} precision={0} suffix="%" onChange={(v) => set({ spacing: v / 100 })} />
          {!eraser && (
            <NumberInput label="Smooth" title="Stabilizer: the stroke follows the pointer with a lag, for steadier lines" value={pct(b.smoothing)} min={0} max={95} precision={0} suffix="%" onChange={(v) => set({ smoothing: v / 100 })} />
          )}
        </div>
        <div className="row-actions">
          <label className="check" title="Pen pressure changes the size">
            <input type="checkbox" checked={b.pressureSize} onChange={(e) => set({ pressureSize: e.target.checked })} />
            Pressure size
          </label>
          <label className="check" title="Pen pressure changes the flow">
            <input type="checkbox" checked={b.pressureFlow} onChange={(e) => set({ pressureFlow: e.target.checked })} />
            Pressure flow
          </label>
        </div>
      </Section>

      <Section
        title="Paint layer"
        actions={
          ui.canEdit && (
            <IconButton
              active={false}
              title="New paint layer (sized to the selected frame, or the view)"
              onClick={() => {
                store.engine.new_paint_layer();
                store.poll();
              }}
            >
              <Plus size={14} />
            </IconButton>
          )
        }
      >
        {layer ? (
          <>
            <div className="paint-layer-name">{layer.name}</div>
            <div className="grid2">
              <NumberInput
                label="Opacity"
                value={pct(layer.opacity)}
                min={0}
                max={100}
                precision={0}
                suffix="%"
                onChange={(v) => setLayer({ opacity: v / 100 })}
                onScrubStart={() => store.engine.begin('edit')}
                onScrubEnd={() => {
                  store.engine.end();
                  store.poll();
                }}
              />
            </div>
            <label className="check" title="Paint only where the layer already has pixels">
              <input type="checkbox" checked={layer.alphaLock === true} onChange={(e) => setLayer({ alphaLock: e.target.checked })} />
              Lock transparent pixels
            </label>
          </>
        ) : (
          <p className="panel-hint">
            {eraser
              ? 'Select a paint layer to erase on it.'
              : 'Select a paint layer, or start painting: a new layer is made, sized to the frame under the brush.'}
          </p>
        )}
      </Section>
    </div>
  );
}
