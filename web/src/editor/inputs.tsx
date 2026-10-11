// Compact property inputs: scrubbable numbers, colors, selects, toggles.
import { useEffect, useRef, useState, type ReactNode } from 'react';

const fmt = (v: number, precision = 2) => {
  if (!Number.isFinite(v)) return '';
  const p = 10 ** precision;
  return String(Math.round(v * p) / p);
};

interface NumberInputProps {
  label: ReactNode;
  value: number | 'mixed';
  onChange: (v: number) => void;
  /** Called at the start/end of a scrub so the drag is one undo step. */
  onScrubStart?: () => void;
  onScrubEnd?: () => void;
  min?: number;
  max?: number;
  step?: number;
  precision?: number;
  suffix?: string;
  title?: string;
  disabled?: boolean;
  className?: string;
}

/** Number field whose label can be dragged horizontally to scrub the value. */
export function NumberInput({
  label,
  value,
  onChange,
  onScrubStart,
  onScrubEnd,
  min = -1e9,
  max = 1e9,
  step = 1,
  precision = 2,
  suffix,
  title,
  disabled,
  className,
}: NumberInputProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const shown = draft ?? (value === 'mixed' ? 'Mixed' : fmt(value, precision) + (suffix ?? ''));
  const scrub = useRef<{ x: number; v: number; moved: boolean } | null>(null);
  const clamp = (v: number) => Math.min(max, Math.max(min, v));

  const commit = (text: string) => {
    setDraft(null);
    const cleaned = text.replace(suffix ?? '', '').trim();
    if (!cleaned || cleaned === 'Mixed') return;
    let v: number;
    try {
      // Allow simple math like "100/2" or "+10".
      if (!/^[-+*/().\d\s]+$/.test(cleaned)) return;
      const base = value === 'mixed' ? 0 : value;
      const expr = /^[+*/]/.test(cleaned) ? `${base}${cleaned}` : cleaned;
      v = Number(Function(`"use strict"; return (${expr});`)());
    } catch {
      return;
    }
    if (Number.isFinite(v)) onChange(clamp(v));
  };

  return (
    <label className={`num-input${disabled ? ' disabled' : ''}${className ? ` ${className}` : ''}`} title={title}>
      <span
        className="num-label"
        onPointerDown={(e) => {
          if (disabled || value === 'mixed') return;
          e.preventDefault();
          (e.target as HTMLElement).setPointerCapture(e.pointerId);
          scrub.current = { x: e.clientX, v: value, moved: false };
        }}
        onPointerMove={(e) => {
          const s = scrub.current;
          if (!s) return;
          const dx = e.clientX - s.x;
          if (!s.moved && Math.abs(dx) < 2) return;
          if (!s.moved) {
            s.moved = true;
            onScrubStart?.();
          }
          const mult = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
          onChange(clamp(Math.round((s.v + dx * step * mult) * 100) / 100));
        }}
        onPointerUp={() => {
          if (scrub.current?.moved) onScrubEnd?.();
          scrub.current = null;
        }}
      >
        {label}
      </span>
      <input
        value={shown}
        disabled={disabled}
        onFocus={(e) => {
          setDraft(value === 'mixed' ? '' : fmt(value, precision));
          requestAnimationFrame(() => e.target.select());
        }}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            commit(e.currentTarget.value);
            e.currentTarget.blur();
          } else if (e.key === 'Escape') {
            setDraft(null);
            e.currentTarget.blur();
          } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
            e.preventDefault();
            const base = value === 'mixed' ? 0 : value;
            const d = (e.key === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? 10 : 1) * step;
            const v = clamp(base + d);
            onChange(v);
            setDraft(fmt(v, precision));
          }
        }}
      />
    </label>
  );
}

const HEX = /^#?([0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;

export function normalizeHex(input: string): string | null {
  const s = input.trim();
  if (!HEX.test(s)) return null;
  let h = s.replace('#', '').toLowerCase();
  if (h.length === 3 || h.length === 4) h = h.split('').map((c) => c + c).join('');
  return `#${h}`;
}

export function splitAlpha(hex: string): { rgb: string; alpha: number } {
  const h = normalizeHex(hex) ?? '#000000';
  if (h.length === 9) return { rgb: h.slice(0, 7), alpha: parseInt(h.slice(7, 9), 16) / 255 };
  return { rgb: h, alpha: 1 };
}

export function withAlpha(rgb: string, alpha: number): string {
  const a = Math.round(Math.min(1, Math.max(0, alpha)) * 255);
  return a >= 255 ? rgb : `${rgb}${a.toString(16).padStart(2, '0')}`;
}

interface ColorInputProps {
  value: string;
  onChange: (hex: string) => void;
  onPickStart?: () => void;
  onPickEnd?: () => void;
}

/** Swatch (native color picker) + hex field + alpha %. */
export function ColorInput({ value, onChange, onPickStart, onPickEnd }: ColorInputProps) {
  const { rgb, alpha } = splitAlpha(value);
  const [hexDraft, setHexDraft] = useState<string | null>(null);
  const picking = useRef(false);
  useEffect(() => setHexDraft(null), [value]);
  return (
    <div className="color-input">
      <label className="swatch" style={{ ['--swatch' as string]: value }}>
        <input
          type="color"
          value={rgb}
          onInput={(e) => {
            if (!picking.current) {
              picking.current = true;
              onPickStart?.();
            }
            onChange(withAlpha(e.currentTarget.value, alpha));
          }}
          onChange={() => {
            if (picking.current) {
              picking.current = false;
              onPickEnd?.();
            }
          }}
          onBlur={() => {
            if (picking.current) {
              picking.current = false;
              onPickEnd?.();
            }
          }}
        />
      </label>
      <input
        className="hex"
        value={hexDraft ?? rgb.replace('#', '').toUpperCase()}
        onChange={(e) => setHexDraft(e.target.value)}
        onBlur={(e) => {
          const h = normalizeHex(e.target.value);
          if (h) onChange(h.length === 9 ? h : withAlpha(h, alpha));
          setHexDraft(null);
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur();
          if (e.key === 'Escape') {
            setHexDraft(null);
            e.currentTarget.blur();
          }
        }}
      />
      <NumberInput
        className="alpha"
        label=""
        value={Math.round(alpha * 100)}
        min={0}
        max={100}
        precision={0}
        suffix="%"
        onChange={(v) => onChange(withAlpha(rgb, v / 100))}
      />
    </div>
  );
}

interface SelectProps<T extends string> {
  value: T | 'mixed';
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  title?: string;
  className?: string;
}

export function Select<T extends string>({ value, options, onChange, title, className }: SelectProps<T>) {
  return (
    <select className={`prop-select${className ? ` ${className}` : ''}`} value={value} title={title} onChange={(e) => onChange(e.target.value as T)}>
      {value === 'mixed' && <option value="mixed">Mixed</option>}
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

interface IconToggleProps {
  active: boolean;
  onClick: () => void;
  title: string;
  children: ReactNode;
  disabled?: boolean;
}

export function IconButton({ active, onClick, title, children, disabled }: IconToggleProps) {
  return (
    <button type="button" className={`icon-btn${active ? ' active' : ''}`} title={title} aria-label={title} aria-pressed={active} onClick={onClick} disabled={disabled}>
      {children}
    </button>
  );
}

export function Section({ title, actions, children }: { title: ReactNode; actions?: ReactNode; children?: ReactNode }) {
  return (
    <section className="prop-section">
      <header>
        <h3>{title}</h3>
        {actions && <div className="section-actions">{actions}</div>}
      </header>
      {children}
    </section>
  );
}
