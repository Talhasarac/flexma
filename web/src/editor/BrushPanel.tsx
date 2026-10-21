// Design panel while the brush or eraser is active: brush settings, presets
// and the paint layer being painted on.
import { useState } from 'react';
import { Pipette, Plus } from 'lucide-react';
import { applyCurve } from './pen';
import { useSelection, useUi, type EditorStore } from './store';
import { ColorInput, IconButton, NumberInput, Section } from './inputs';
import type { BrushSettings, NodeData } from './types';

type Preset = { name: string; brush: Partial<BrushSettings> };

/** Every textured setting at "off", so a preset never inherits another's texture. */
const PLAIN: Partial<BrushSettings> = {
  tip: null,
  tipColor: false,
  angle: 0,
  follow: 0,
  sizeJitter: 0,
  angleJitter: 0,
  flowJitter: 0,
  scatter: 0,
  grain: null,
  grainScale: 1,
  grainStrength: 0,
  tint: null,
  tintScale: 1,
  tintStrength: 0,
  smudge: 0,
  mix: 0,
  tiltSize: 0,
  tiltFlow: 0,
  tiltAngle: false,
};

const preset = (name: string, brush: Partial<BrushSettings>): Preset => ({ name, brush: { ...PLAIN, ...brush } });

const PRESET_GROUPS: { label: string; presets: Preset[] }[] = [
  {
    label: 'Basic',
    presets: [
      preset('Round', { size: 12, hardness: 0.8, flow: 1, spacing: 0.1, pressureSize: true, pressureFlow: false, smoothing: 0.35 }),
      preset('Pencil', { size: 4, hardness: 0.9, flow: 0.9, spacing: 0.08, pressureSize: true, pressureFlow: true, smoothing: 0.2, tiltSize: 0.6, tiltFlow: 0.4 }),
      preset('Ink', { size: 8, hardness: 1, flow: 1, spacing: 0.06, pressureSize: true, pressureFlow: false, smoothing: 0.55 }),
      preset('Marker', { size: 24, hardness: 0.7, flow: 0.3, spacing: 0.08, pressureSize: false, pressureFlow: true, smoothing: 0.3 }),
      preset('Airbrush', { size: 90, hardness: 0, flow: 0.05, spacing: 0.04, pressureSize: false, pressureFlow: true, smoothing: 0.2 }),
    ],
  },
  {
    label: 'Dry',
    presets: [
      preset('Chalk', {
        tip: 'chalk-1', size: 40, flow: 0.85, spacing: 0.08, angleJitter: 180, sizeJitter: 0.08, pressureSize: true, pressureFlow: false, smoothing: 0.25,
        grain: 'paper-1', grainScale: 1, grainStrength: 0.8, tiltSize: 0.5, tiltFlow: 0.3,
      }),
      preset('Soft chalk', {
        tip: 'chalk-1', size: 60, flow: 0.3, spacing: 0.08, angleJitter: 180, pressureSize: true, pressureFlow: true, smoothing: 0.3,
        grain: 'paper-1', grainScale: 1, grainStrength: 0.35, tint: 'colour-1', tintScale: 1.5, tintStrength: 0.2, tiltSize: 0.6, tiltFlow: 0.3,
      }),
      preset('Rough pencil', {
        tip: 'scribble-1', size: 14, flow: 0.9, spacing: 0.12, angleJitter: 180, sizeJitter: 0.15, pressureSize: true, pressureFlow: true, smoothing: 0.2,
        grain: 'noise-1', grainScale: 0.5, grainStrength: 0.5, tiltSize: 0.6, tiltFlow: 0.4,
      }),
      preset('Charcoal', {
        tip: 'charcoal-1', size: 36, flow: 0.8, spacing: 0.08, angleJitter: 180, sizeJitter: 0.1, pressureSize: true, pressureFlow: true, smoothing: 0.2,
        grain: 'paper-1', grainScale: 1, grainStrength: 0.9, tiltSize: 0.7, tiltFlow: 0.3,
      }),
      preset('Cloud', {
        tip: 'cloud-1', size: 140, flow: 0.45, spacing: 0.22, angleJitter: 25, sizeJitter: 0.3, flowJitter: 0.3, scatter: 0.12, pressureSize: false, pressureFlow: true, smoothing: 0.3,
      }),
    ],
  },
  {
    label: 'Paint',
    presets: [
      preset('Flat brush', {
        // The bristles lie across the stroke and leave streaks along it.
        tip: 'bristle-1', size: 50, flow: 0.9, spacing: 0.03, follow: 1, pressureSize: true, pressureFlow: false, smoothing: 0.4,
        tint: 'colour-1', tintScale: 1.5, tintStrength: 0.15,
      }),
      preset('Heavy roller', {
        tip: 'roller-1', size: 70, flow: 0.9, spacing: 0.04, follow: 1, pressureSize: false, pressureFlow: true, smoothing: 0.4,
        grain: 'drip-1', grainScale: 1, grainStrength: 0.55, tint: 'colour-1', tintScale: 2, tintStrength: 0.3,
      }),
      preset('Palette knife', {
        // The blade lies across the stroke and lays a band; a tilted pen turns it.
        tip: 'knife-1', angle: 90, size: 56, tiltAngle: true, flow: 1, spacing: 0.035, follow: 1, pressureSize: true, pressureFlow: false, smoothing: 0.4,
        grain: 'drip-1', grainScale: 0.8, grainStrength: 0.4, tint: 'colour-1', tintScale: 1.5, tintStrength: 0.35,
      }),
      preset('Soft knife', {
        // The blade lies across the stroke and lays a band; a tilted pen turns it.
        tip: 'knife-1', angle: 90, size: 56, tiltAngle: true, flow: 0.45, spacing: 0.035, follow: 1, pressureSize: true, pressureFlow: true, smoothing: 0.4,
        grain: 'drip-1', grainScale: 0.8, grainStrength: 0.65, tint: 'colour-1', tintScale: 1.5, tintStrength: 0.35,
      }),
      preset('Sponge', {
        tip: 'sponge-1', size: 80, flow: 0.7, spacing: 0.3, angleJitter: 180, sizeJitter: 0.2, flowJitter: 0.3, scatter: 0.15, pressureSize: false, pressureFlow: true, smoothing: 0.2,
        tint: 'colour-1', tintScale: 1, tintStrength: 0.15,
      }),
    ],
  },
  {
    label: 'Effects',
    presets: [
      preset('Spray paint', {
        tip: 'spray-1', size: 80, flow: 0.3, spacing: 0.12, angleJitter: 180, sizeJitter: 0.1, pressureSize: false, pressureFlow: true, smoothing: 0.2,
      }),
      preset('Splatter', {
        tip: 'splatter-1', size: 110, flow: 1, spacing: 1.2, angleJitter: 180, sizeJitter: 0.5, scatter: 0.8, pressureSize: true, pressureFlow: false, smoothing: 0.1,
      }),
      preset('Sparkles', {
        tip: 'sparkle-1', size: 70, flow: 1, spacing: 1.5, angleJitter: 10, sizeJitter: 0.7, scatter: 1.5, pressureSize: true, pressureFlow: false, smoothing: 0.1,
      }),
    ],
  },
  {
    label: 'Nature',
    presets: [
      preset('Grass', {
        // Tufts stay upright (no follow); the color varies across the layer.
        tip: 'grass-1', size: 60, flow: 1, spacing: 0.25, angleJitter: 15, sizeJitter: 0.35, scatter: 0.25, pressureSize: true, pressureFlow: false, smoothing: 0.2,
        tint: 'colour-1', tintScale: 1, tintStrength: 0.25,
      }),
      preset('Leaves', {
        tip: 'leaf-1', size: 40, flow: 1, spacing: 0.6, angleJitter: 180, sizeJitter: 0.4, flowJitter: 0.15, scatter: 0.8, pressureSize: true, pressureFlow: false, smoothing: 0.2,
        tint: 'colour-1', tintScale: 1, tintStrength: 0.3,
      }),
    ],
  },
  {
    label: 'Tone',
    presets: [
      // Round brushes that paint only a print screen fixed to the layer, so
      // overlapping strokes line up.
      preset('Halftone', {
        size: 60, hardness: 0.9, flow: 1, spacing: 0.1, pressureSize: false, pressureFlow: false, smoothing: 0.3,
        grain: 'halftone-1', grainScale: 1, grainStrength: 1,
      }),
      preset('Hatching', {
        size: 60, hardness: 0.9, flow: 1, spacing: 0.1, pressureSize: false, pressureFlow: true, smoothing: 0.3,
        grain: 'hatch-1', grainScale: 1, grainStrength: 1,
      }),
    ],
  },
  {
    label: 'Blend',
    presets: [
      preset('Smudge', {
        tip: 'chalk-1', size: 60, flow: 0.55, spacing: 0.06, angleJitter: 180, pressureSize: false, pressureFlow: true, smoothing: 0.3,
        grain: 'paper-1', grainStrength: 0.25, smudge: 0.85,
      }),
      preset('Smudge stick', {
        tip: 'chalk-1', size: 40, flow: 0.6, spacing: 0.05, angleJitter: 180, pressureSize: true, pressureFlow: false, smoothing: 0.3,
        grain: 'paper-1', grainStrength: 0.1, smudge: 0.93,
      }),
      preset('Mixer chalk', {
        tip: 'chalk-1', size: 40, flow: 0.85, spacing: 0.06, angleJitter: 180, pressureSize: true, pressureFlow: false, smoothing: 0.3,
        grain: 'paper-1', grainStrength: 0.5, smudge: 0.75, mix: 0.18,
      }),
      preset('Mixer knife', {
        tip: 'speckle-1', size: 60, flow: 0.9, spacing: 0.04, follow: 1, pressureSize: true, pressureFlow: false, smoothing: 0.4,
        smudge: 0.8, mix: 0.2,
      }),
      preset('Mixer brush', {
        // The flat brush, pulling the wet paint under it along.
        tip: 'bristle-1', size: 50, flow: 0.9, spacing: 0.04, follow: 1, pressureSize: true, pressureFlow: false, smoothing: 0.4,
        smudge: 0.8, mix: 0.2,
      }),
    ],
  },
];

const ERASER_GROUPS: { label: string; presets: Preset[] }[] = [
  {
    label: 'Erasers',
    presets: [
      preset('Hard', { size: 20, hardness: 0.95, flow: 1, spacing: 0.08, pressureSize: false, pressureFlow: false }),
      preset('Soft', { size: 60, hardness: 0.1, flow: 0.4, spacing: 0.06, pressureSize: false, pressureFlow: true }),
      preset('Airbrush eraser', { size: 90, hardness: 0, flow: 0.1, spacing: 0.04, pressureSize: false, pressureFlow: true }),
      preset('Textured eraser', {
        tip: 'chalk-1', size: 40, flow: 0.85, spacing: 0.08, angleJitter: 180, pressureSize: true, pressureFlow: false,
        grain: 'noise-1', grainScale: 0.75, grainStrength: 0.6,
      }),
    ],
  },
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
    return typeof v === 'number' ? Math.abs(v - (b[k] as number)) < 1e-3 : v === (b[k] ?? null);
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
  const groups = eraser ? ERASER_GROUPS : PRESET_GROUPS;
  const pct = (v: number) => Math.round(v * 100);

  const setLayer = (props: Record<string, unknown>) => {
    store.engine.update_selection(JSON.stringify(props));
    store.poll();
  };

  return (
    <div className="props">
      <Section title={eraser ? 'Eraser' : 'Brush'}>
        {groups.map((g) => (
          <div key={g.label} className="brush-group">
            {groups.length > 1 && <div className="brush-group-label">{g.label}</div>}
            <div className="brush-presets">
              {g.presets.map((p) => (
                <button key={p.name} type="button" className={`chip${close(p.brush, b) ? ' active' : ''}`} onClick={() => set(p.brush)}>
                  {p.name}
                </button>
              ))}
            </div>
          </div>
        ))}
        {!eraser && !b.tipColor && (
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
          {!b.tip && (
            <NumberInput label="Hard" title="Edge hardness" value={pct(b.hardness)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ hardness: v / 100 })} />
          )}
          <NumberInput label="Flow" title="How much paint each dab lays down; overlapping dabs build up" value={pct(b.flow)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ flow: v / 100 })} />
          <NumberInput label="Gap" title="Spacing between dabs, as a share of the size" value={pct(b.spacing)} min={2} max={200} precision={0} suffix="%" onChange={(v) => set({ spacing: v / 100 })} />
          {!eraser && (
            <NumberInput label="Smooth" title="Stabilizer: the stroke follows the pointer with a lag, for steadier lines" value={pct(b.smoothing)} min={0} max={95} precision={0} suffix="%" onChange={(v) => set({ smoothing: v / 100 })} />
          )}
          {b.grain && (
            <NumberInput label="Grain" title="How much the paper grain shows" value={pct(b.grainStrength)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ grainStrength: v / 100 })} />
          )}
          {b.tint && (
            <NumberInput label="Vary" title="Color variation across the canvas" value={pct(b.tintStrength)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ tintStrength: v / 100 })} />
          )}
          {b.smudge > 0 && (
            <NumberInput label="Smudge" title="How far the paint is dragged" value={pct(b.smudge)} min={1} max={100} precision={0} suffix="%" onChange={(v) => set({ smudge: v / 100 })} />
          )}
          {b.smudge > 0 && (
            <NumberInput label="Mix" title="How much of the brush color goes into the smudge" value={pct(b.mix)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ mix: v / 100 })} />
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
        <div className="grid2 brush-tilt">
          <NumberInput label="Tilt" title="Tilting the pen widens the dab (up to 3× when flat), like shading with the side of a pencil" value={pct(b.tiltSize)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ tiltSize: v / 100 })} />
          <NumberInput label="Tilt fade" title="Tilting the pen makes the dab lighter" value={pct(b.tiltFlow)} min={0} max={100} precision={0} suffix="%" onChange={(v) => set({ tiltFlow: v / 100 })} />
        </div>
        <label className="check" title="The tip turns to point the way the pen leans">
          <input type="checkbox" checked={b.tiltAngle} onChange={(e) => set({ tiltAngle: e.target.checked })} />
          Tip follows pen
        </label>
      </Section>

      <PenSection store={store} />

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

/** The pen: its pressure curve (per browser, for every brush). */
function PenSection({ store }: { store: EditorStore }) {
  const [curve, setCurve] = useState(store.pressureCurve);
  const points = Array.from({ length: 21 }, (_, i) => {
    const p = i / 20;
    return `${(p * 100).toFixed(1)},${(100 - applyCurve(p, curve) * 100).toFixed(1)}`;
  }).join(' ');
  return (
    <Section title="Pen">
      <div className="pen-curve">
        <svg viewBox="-4 -4 108 108" aria-hidden="true">
          <rect x="0" y="0" width="100" height="100" />
          <polyline points={points} />
        </svg>
        <div className="pen-curve-control">
          <label className="field-label" htmlFor="pen-curve">
            Pressure curve
          </label>
          <input
            id="pen-curve"
            type="range"
            min={-100}
            max={100}
            step={5}
            value={Math.round(curve * 100)}
            onChange={(e) => {
              const v = Number(e.target.value) / 100;
              setCurve(v);
              store.setPressureCurve(v);
            }}
          />
          <div className="pen-curve-ends">
            <span>Light touch</span>
            <span>Firm</span>
          </div>
        </div>
      </div>
      <p className="panel-hint">Pens (Wacom, Apple Pencil, Surface) paint with pressure and tilt. Flip the stylus to erase.</p>
    </Section>
  );
}
