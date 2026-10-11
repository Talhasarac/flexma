// Font helpers shared by the editor and workspace settings: registering
// workspace fonts with the browser, the Google Fonts catalogue and previews,
// and fonts installed on this computer.
import { api, type GoogleFontFamily, type WorkspaceFont } from './api';

/** Always available: bundled with Flexma. */
export const BUILTIN_FONTS = ['Inter'];

const GENERIC_FAMILIES = new Set([
  'system-ui',
  'sans-serif',
  'serif',
  'monospace',
  'cursive',
  'fantasy',
  'ui-sans-serif',
  'ui-serif',
  'ui-monospace',
  '-apple-system',
]);

const WEIGHT_NAMES: Record<number, string> = {
  100: 'Thin',
  200: 'Extra Light',
  300: 'Light',
  400: 'Regular',
  500: 'Medium',
  600: 'Semibold',
  700: 'Bold',
  800: 'Extra Bold',
  900: 'Black',
};

export const WEIGHTS = [100, 200, 300, 400, 500, 600, 700, 800, 900];

export function weightName(weight: number): string {
  return WEIGHT_NAMES[Math.round(weight / 100) * 100] ?? String(weight);
}

export function styleName(weight: number, italic: boolean): string {
  const w = weightName(weight);
  if (!italic) return w;
  return w === 'Regular' ? 'Italic' : `${w} Italic`;
}

/** "700i" → { weight: 700, italic: true } (Google Fonts style keys). */
export function parseStyle(s: string): { weight: number; italic: boolean } {
  return { weight: parseInt(s, 10) || 400, italic: s.endsWith('i') };
}

export function sameFamily(a: string, b: string): boolean {
  return a.localeCompare(b, undefined, { sensitivity: 'accent' }) === 0;
}

export function groupByFamily(faces: WorkspaceFont[]): Map<string, WorkspaceFont[]> {
  const out = new Map<string, WorkspaceFont[]>();
  for (const f of faces) {
    const list = out.get(f.family);
    if (list) list.push(f);
    else out.set(f.family, [f]);
  }
  return out;
}

/** The face closest to a weight and style. */
export function closestFace(faces: WorkspaceFont[], weight: number, italic: boolean): WorkspaceFont | undefined {
  const sameStyle = faces.filter((f) => f.italic === italic);
  const pool = sameStyle.length ? sameStyle : faces;
  let best: WorkspaceFont | undefined;
  let bestDist = Infinity;
  for (const f of pool) {
    const d = weight >= f.weight && weight <= f.weightMax ? 0 : Math.min(Math.abs(weight - f.weight), Math.abs(weight - f.weightMax));
    if (d < bestDist) {
      best = f;
      bestDist = d;
    }
  }
  return best;
}

/** Weights a family offers, for the weight menu. */
export function familyWeights(faces: WorkspaceFont[]): number[] {
  const set = new Set<number>();
  for (const f of faces) for (const w of WEIGHTS) if (w >= f.weight && w <= f.weightMax) set.add(w);
  return [...set].sort((a, b) => a - b);
}

// ---------------------------------------------------------------------------
// Registering workspace fonts with the browser
// ---------------------------------------------------------------------------

const registered = new Map<string, FontFace>();

/** The (lazily loading) FontFace for a workspace face served at `url`. */
export function fontFaceFor(f: WorkspaceFont, url: string): FontFace {
  let face = registered.get(url);
  if (!face) {
    face = new FontFace(f.family, `url("${url}") format("${f.format === 'otf' ? 'opentype' : 'truetype'}")`, {
      weight: f.weightMax > f.weight ? `${f.weight} ${f.weightMax}` : String(f.weight),
      style: f.italic ? 'italic' : 'normal',
    });
    document.fonts.add(face);
    registered.set(url, face);
  }
  return face;
}

export function forgetFontFace(url: string) {
  const face = registered.get(url);
  if (face) {
    document.fonts.delete(face);
    registered.delete(url);
  }
}

// ---------------------------------------------------------------------------
// Google Fonts
// ---------------------------------------------------------------------------

let catalogue: Promise<GoogleFontFamily[]> | null = null;

/** The Google Fonts catalogue (fetched once per page load, via Flexma). */
export function googleCatalogue(): Promise<GoogleFontFamily[]> {
  catalogue ??= api.fonts.googleCatalogue().catch((err: unknown) => {
    catalogue = null;
    throw err;
  });
  return catalogue;
}

export function findGoogleFamily(list: GoogleFontFamily[] | null, family: string): GoogleFontFamily | undefined {
  return list?.find((f) => sameFamily(f.family, family));
}

const previews = new Map<string, Promise<string>>();

/**
 * Load a tiny preview font (just the family name's glyphs) through Flexma and
 * resolve to the CSS family to show it with.
 */
export function googlePreviewFamily(family: string): Promise<string> {
  let p = previews.get(family);
  if (!p) {
    const alias = `Flexma Preview ${family}`;
    p = new FontFace(alias, `url("${api.fonts.googlePreviewUrl(family)}")`).load().then((face) => {
      document.fonts.add(face);
      return alias;
    });
    p.catch(() => previews.delete(family));
    previews.set(family, p);
  }
  return p;
}

// ---------------------------------------------------------------------------
// Fonts on this computer
// ---------------------------------------------------------------------------

const detected = new Map<string, boolean>();
let measureCtx: CanvasRenderingContext2D | null = null;

/** Whether this browser can draw a family without a web font (installed, or generic). */
export function isInstalled(family: string): boolean {
  const key = family.toLowerCase();
  if (GENERIC_FAMILIES.has(key)) return true;
  const known = detected.get(key);
  if (known !== undefined) return known;
  measureCtx ??= document.createElement('canvas').getContext('2d');
  if (!measureCtx) return true;
  const sample = 'mmmmmmmmmmlli1WQ@#&wy';
  let found = false;
  for (const base of ['monospace', 'serif', 'sans-serif']) {
    measureCtx.font = `72px ${base}`;
    const fallback = measureCtx.measureText(sample).width;
    measureCtx.font = `72px "${family.replace(/"/g, '')}", ${base}`;
    if (Math.abs(measureCtx.measureText(sample).width - fallback) > 0.5) {
      found = true;
      break;
    }
  }
  detected.set(key, found);
  return found;
}

interface LocalFontData {
  family: string;
  style: string;
  fullName: string;
  blob(): Promise<Blob>;
}

export interface LocalFamily {
  family: string;
  faces: LocalFontData[];
}

/** Chrome and Edge can list installed fonts (after asking permission). */
export function localFontsSupported(): boolean {
  return typeof window !== 'undefined' && 'queryLocalFonts' in window;
}

export async function queryLocalFamilies(): Promise<LocalFamily[]> {
  const query = (window as unknown as { queryLocalFonts: () => Promise<LocalFontData[]> }).queryLocalFonts;
  const fonts = await query();
  const map = new Map<string, LocalFontData[]>();
  for (const f of fonts) {
    const list = map.get(f.family);
    if (list) list.push(f);
    else map.set(f.family, [f]);
  }
  return [...map.entries()]
    .map(([family, faces]) => ({ family, faces }))
    .sort((a, b) => a.family.localeCompare(b.family));
}

export function formatBytes(n: number): string {
  if (n < 1024 * 1024) return `${Math.max(1, Math.round(n / 1024))} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
