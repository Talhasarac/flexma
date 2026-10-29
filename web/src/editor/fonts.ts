// FontManager: makes the workspace's fonts available to the editor. It loads
// only the faces a file actually uses, keeps the engine from measuring text
// while those are still loading (so text boxes aren't sized for a fallback
// font), re-measures text once they arrive, and tracks fonts the file uses
// that nobody can see.
import { api, type WorkspaceFont } from '../lib/api';
import { BUILTIN_FONTS, closestFace, fontFaceFor, forgetFontFace, groupByFamily, isInstalled, sameFamily } from '../lib/fonts';
import type { EditorStore } from './store';

export interface UsedFont {
  family: string;
  /** [weight, italic] pairs used by text in the file. */
  styles: [number, boolean][];
  /** Text layers using the family. */
  count: number;
}

export interface FontState {
  faces: WorkspaceFont[];
  families: Map<string, WorkspaceFont[]>;
  used: UsedFont[];
  /** Used fonts this browser can't show (it draws a fallback instead). */
  missing: UsedFont[];
}

export class FontManager {
  state: FontState = { faces: [], families: new Map(), used: [], missing: [] };
  private store: EditorStore;
  private timer = 0;
  private readyTimer = 0;
  private browserReady = false;
  private listed = false;
  private pending = 0;

  constructor(store: EditorStore) {
    this.store = store;
    // Hold text measurement until the workspace's fonts are known and loaded.
    store.engine.fonts_loading();
  }

  getState = () => this.state;

  private url(f: WorkspaceFont): string {
    const token = this.store.shareToken;
    return token ? api.fonts.shareFileUrl(token, f.id) : api.fonts.fileUrl(f.id);
  }

  private list(): Promise<WorkspaceFont[]> {
    const { shareToken, workspaceId } = this.store;
    if (shareToken) return api.fonts.shareList(shareToken);
    if (workspaceId) return api.fonts.list(workspaceId);
    return Promise.resolve([]);
  }

  /** The browser finished loading some fonts (document.fonts events). Debounced:
   * the font picker loads many preview fonts, and each redraw resets text. */
  browserFontsLoaded() {
    window.clearTimeout(this.readyTimer);
    this.readyTimer = window.setTimeout(() => {
      this.browserReady = true;
      this.syncEngine();
    }, this.browserReady ? 120 : 0);
  }

  /** Let the engine measure text only when nothing is still loading. */
  private syncEngine() {
    const e = this.store.engine;
    if (this.browserReady && this.listed && this.pending === 0) e.fonts_ready();
    else e.fonts_loading();
  }

  /** Fetch the workspace's fonts (on open, and when they change). */
  async refresh() {
    let faces: WorkspaceFont[];
    try {
      faces = await this.list();
    } catch {
      faces = this.state.faces;
    }
    const keep = new Set(faces.map((f) => this.url(f)));
    const had = new Set(this.state.faces.map((f) => this.url(f)));
    for (const f of this.state.faces) if (!keep.has(this.url(f))) forgetFontFace(this.url(f));
    for (const f of faces) fontFaceFor(f, this.url(f));
    // A face came or went: text drawn so far may use the wrong font, so the
    // engine redraws it when the check below reports the fonts ready again.
    if (this.listed && (keep.size !== had.size || [...keep].some((u) => !had.has(u)))) this.store.engine.fonts_changed();
    this.state = { ...this.state, faces, families: groupByFamily(faces) };
    this.listed = true;
    await this.ensureUsed();
  }

  /** Re-check used fonts soon (the document changed). */
  schedule() {
    window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => void this.ensureUsed(), 150);
  }

  family(name: string): WorkspaceFont[] | undefined {
    const exact = this.state.families.get(name);
    if (exact) return exact;
    for (const [f, faces] of this.state.families) if (sameFamily(f, name)) return faces;
    return undefined;
  }

  isAvailable(family: string): boolean {
    return !!this.family(family) || BUILTIN_FONTS.some((b) => sameFamily(b, family)) || isInstalled(family);
  }

  /** Load the faces the file uses; once in, re-measure their text. */
  async ensureUsed() {
    const e = this.store.engine;
    let used: UsedFont[];
    try {
      used = JSON.parse(e.used_fonts()) as UsedFont[];
    } catch {
      return;
    }
    const loads: Promise<unknown>[] = [];
    const families = new Set<string>();
    for (const u of used) {
      const faces = this.family(u.family);
      if (!faces) continue;
      for (const [weight, italic] of u.styles) {
        const f = closestFace(faces, weight, italic);
        if (!f) continue;
        const face = fontFaceFor(f, this.url(f));
        if (face.status !== 'loaded') {
          loads.push(face.load());
          families.add(u.family);
        }
      }
    }
    if (loads.length) {
      this.pending++;
      this.syncEngine();
      await Promise.allSettled(loads);
      this.pending--;
    }
    this.syncEngine();
    if (families.size && this.pending === 0) e.remeasure_fonts(JSON.stringify([...families]));
    this.state = { ...this.state, used, missing: used.filter((u) => !this.isAvailable(u.family)) };
    this.store.notify();
    this.store.poll();
  }

  /** Load the styles about to be applied, so text is measured with the real font. */
  async prepare(family: string, styles: [number, boolean][]) {
    const faces = this.family(family);
    if (!faces) return;
    const loads = styles
      .map(([w, i]) => closestFace(faces, w, i))
      .filter((f): f is WorkspaceFont => !!f)
      .map((f) => fontFaceFor(f, this.url(f)).load());
    await Promise.allSettled(loads);
  }

  /** Load a family's regular face (for previews). Resolves false if it isn't a workspace font. */
  async preview(family: string): Promise<boolean> {
    const faces = this.family(family);
    const f = faces && closestFace(faces, 400, false);
    if (!f) return false;
    await fontFaceFor(f, this.url(f))
      .load()
      .catch(() => undefined);
    return true;
  }
}
