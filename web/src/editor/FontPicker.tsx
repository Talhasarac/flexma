// Font picker: workspace fonts, Google Fonts (added to the workspace on
// pick, so everyone gets them), fonts on this computer and uploads. Each row
// previews the family in its own typeface.
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, Check, ChevronDown, CloudUpload, Loader2, Search, Upload } from 'lucide-react';
import { api, errorMessage, type GoogleFontFamily } from '../lib/api';
import {
  BUILTIN_FONTS,
  googleCatalogue,
  googlePreviewFamily,
  localFontsSupported,
  queryLocalFamilies,
  sameFamily,
  type LocalFamily,
} from '../lib/fonts';
import { useToast } from '../components/Toast';
import { useFontState, useUi, type EditorStore } from './store';

type Kind = 'used' | 'workspace' | 'builtin' | 'google' | 'local';

const CATEGORIES = [
  { id: '', label: 'All' },
  { id: 'Sans Serif', label: 'Sans' },
  { id: 'Serif', label: 'Serif' },
  { id: 'Display', label: 'Display' },
  { id: 'Handwriting', label: 'Script' },
  { id: 'Monospace', label: 'Mono' },
];

const MAX_ROWS = 150;

interface FontPickerProps {
  store: EditorStore;
  value: string;
  /** Styles to load before `onPick`, so text is measured with the real font. */
  styles?: [number, boolean][];
  disabled?: boolean;
  onPick: (family: string) => void;
  /** Smaller trigger (e.g. inside dialogs). */
  compact?: boolean;
  placeholder?: string;
}

export function FontPicker({ store, value, styles, disabled, onPick, compact, placeholder = 'Mixed' }: FontPickerProps) {
  useFontState(store); // re-render as fonts load
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const known = !!value && store.fonts.isAvailable(value);
  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className={`font-trigger${compact ? ' compact' : ''}`}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={value ? (known ? value : `${value} isn't available here; a fallback font is shown`) : undefined}
      >
        <span className="font-trigger-name" style={value && known ? { fontFamily: `"${value}", Inter, sans-serif` } : undefined}>
          {value || placeholder}
        </span>
        {value && !known && <AlertTriangle size={12} className="font-warn-icon" aria-label="Font not available" />}
        <ChevronDown size={12} />
      </button>
      {open && triggerRef.current && (
        <FontPopover
          store={store}
          anchor={triggerRef.current}
          value={value}
          styles={styles}
          onClose={() => setOpen(false)}
          onPick={(f) => {
            setOpen(false);
            onPick(f);
          }}
        />
      )}
    </>
  );
}

function FontPopover({
  store,
  anchor,
  value,
  styles,
  onClose,
  onPick,
}: {
  store: EditorStore;
  anchor: HTMLElement;
  value: string;
  styles?: [number, boolean][];
  onClose: () => void;
  onPick: (family: string) => void;
}) {
  const toast = useToast();
  const ui = useUi(store);
  const fonts = useFontState(store);
  const canAdd = ui.canEdit && !!store.workspaceId;
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState('');
  const [google, setGoogle] = useState<GoogleFontFamily[] | null>(null);
  const [googleError, setGoogleError] = useState<string | null>(null);
  const [local, setLocal] = useState<LocalFamily[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    googleCatalogue().then(setGoogle, (err: unknown) => setGoogleError(errorMessage(err, 'Google Fonts is unavailable')));
  }, []);

  // Close on outside click.
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!popRef.current?.contains(t) && !anchor.contains(t)) onClose();
    };
    document.addEventListener('mousedown', onDown, true);
    return () => document.removeEventListener('mousedown', onDown, true);
  }, [anchor, onClose]);

  const pos = useMemo(() => {
    const r = anchor.getBoundingClientRect();
    const width = 320;
    const height = Math.min(480, window.innerHeight - 24);
    const left = Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8));
    const below = window.innerHeight - r.bottom - 12;
    const top = below >= height ? r.bottom + 4 : Math.max(8, Math.min(r.top - height - 4, window.innerHeight - height - 8));
    return { left, top, width, height };
  }, [anchor]);

  const q = query.trim().toLowerCase();
  const matches = (f: string) => !q || f.toLowerCase().includes(q);
  const workspaceFamilies = [...fonts.families.keys()];
  const inWorkspace = (f: string) => workspaceFamilies.some((w) => sameFamily(w, f));

  const used = fonts.used.map((u) => u.family).filter(matches);
  const workspace = workspaceFamilies.filter(matches);
  const builtin = BUILTIN_FONTS.filter((f) => matches(f) && !inWorkspace(f));
  const googleRows = (google ?? [])
    .filter((g) => !inWorkspace(g.family) && (!category || g.category === category) && matches(g.family))
    .slice(0, MAX_ROWS);
  const localRows = (local ?? []).filter((l) => !inWorkspace(l.family) && matches(l.family)).slice(0, MAX_ROWS);

  const pick = async (family: string, kind: Kind) => {
    if (busy) return;
    setBusy(family);
    try {
      if (kind === 'google' && !inWorkspace(family)) {
        await api.fonts.addGoogle(store.workspaceId!, family);
        await store.fonts.refresh();
        toast.success(`Added ${family} to the workspace`, { id: 'font-added' });
      }
      await store.fonts.prepare(family, styles?.length ? styles : [[400, false]]);
      onPick(family);
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(null);
    }
  };

  const uploadFiles = async (files: File[]) => {
    if (!store.workspaceId || files.length === 0) return;
    setBusy('upload');
    const added = new Set<string>();
    for (const f of files) {
      try {
        added.add((await api.fonts.upload(store.workspaceId, f)).family);
      } catch (err) {
        toast.error(`${f.name}: ${errorMessage(err)}`);
      }
    }
    await store.fonts.refresh();
    setBusy(null);
    if (added.size) toast.success(`Added ${[...added].join(', ')} to the workspace`, { id: 'font-added' });
    if (added.size === 1) void pick([...added][0], 'workspace');
  };

  const uploadLocal = async (fam: LocalFamily) => {
    if (!store.workspaceId) return;
    setBusy(fam.family);
    let ok = 0;
    let firstError = '';
    for (const face of fam.faces) {
      try {
        await api.fonts.upload(store.workspaceId, await face.blob());
        ok++;
      } catch (err) {
        firstError ||= errorMessage(err);
      }
    }
    await store.fonts.refresh();
    setBusy(null);
    if (ok) toast.success(`Added ${fam.family} (${ok} ${ok === 1 ? 'style' : 'styles'}) to the workspace`, { id: 'font-added' });
    else toast.error(`Couldn't add ${fam.family}: ${firstError}`);
  };

  const showLocal = async () => {
    try {
      setLocal(await queryLocalFamilies());
    } catch (err) {
      toast.error(errorMessage(err, "Couldn't read this computer's fonts"));
    }
  };

  const first = used[0] ?? workspace[0] ?? builtin[0] ?? googleRows[0]?.family ?? localRows[0]?.family;
  const firstKind: Kind = used[0] ? 'used' : workspace[0] ? 'workspace' : builtin[0] ? 'builtin' : googleRows[0] ? 'google' : 'local';

  const row = (family: string, kind: Kind, extra?: ReactNode) => (
    <FontRow
      key={`${kind}:${family}`}
      store={store}
      family={family}
      kind={kind}
      root={listRef}
      selected={!!value && sameFamily(family, value)}
      busy={busy === family}
      disabled={!!busy || (kind === 'google' && !canAdd && !inWorkspace(family))}
      missing={kind === 'used' && !store.fonts.isAvailable(family)}
      onPick={() => void pick(family, kind)}
      extra={extra}
    />
  );

  return createPortal(
    <div
      ref={popRef}
      className="font-popover"
      role="dialog"
      aria-label="Choose a font"
      style={{ left: pos.left, top: pos.top, width: pos.width, maxHeight: pos.height }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="font-search">
        <Search size={13} />
        <input
          autoFocus
          value={query}
          placeholder="Search fonts"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && first) void pick(first, firstKind);
          }}
        />
      </div>
      <div className="font-cats" role="group" aria-label="Google Fonts category">
        {CATEGORIES.map((c) => (
          <button key={c.id} type="button" className={category === c.id ? 'on' : ''} onClick={() => setCategory(c.id)}>
            {c.label}
          </button>
        ))}
      </div>
      <div className="font-list" ref={listRef}>
        {used.length > 0 && <div className="font-group">In this file</div>}
        {used.map((f) => row(f, 'used'))}
        {(workspace.length > 0 || builtin.length > 0) && <div className="font-group">Workspace</div>}
        {workspace.map((f) => row(f, 'workspace'))}
        {builtin.map((f) => row(f, 'builtin'))}
        <div className="font-group">
          Google Fonts
          {!canAdd && google && <span className="font-group-note">editors can add these</span>}
        </div>
        {googleError && <div className="font-empty">{googleError}</div>}
        {!google && !googleError && (
          <div className="font-empty">
            <Loader2 size={13} className="spin" /> Loading…
          </div>
        )}
        {googleRows.map((g) => row(g.family, 'google'))}
        {google && googleRows.length === 0 && <div className="font-empty">No matches</div>}
        {google && !q && !category && <div className="font-empty">Search to see all {google.length.toLocaleString()} families</div>}
        {localFontsSupported() && (
          <>
            <div className="font-group">On this computer</div>
            {local === null ? (
              <button type="button" className="font-local-btn" onClick={() => void showLocal()}>
                Show fonts installed on this computer
              </button>
            ) : (
              <>
                <div className="font-empty">Only you can see these until they're added to the workspace.</div>
                {localRows.map((l) =>
                  row(
                    l.family,
                    'local',
                    canAdd && (
                      <button
                        type="button"
                        className="font-row-action"
                        title={`Add ${l.family} to the workspace so everyone sees it`}
                        onClick={(e) => {
                          e.stopPropagation();
                          void uploadLocal(l);
                        }}
                      >
                        <CloudUpload size={13} />
                      </button>
                    ),
                  ),
                )}
              </>
            )}
          </>
        )}
      </div>
      {canAdd && (
        <div className="font-footer">
          <button type="button" className="btn btn-sm btn-ghost" disabled={!!busy} onClick={() => fileRef.current?.click()}>
            {busy === 'upload' ? <Loader2 size={13} className="spin" /> : <Upload size={13} />} Upload font files
          </button>
          <span className="font-footer-note">.ttf or .otf, shared with the workspace</span>
          <input
            ref={fileRef}
            type="file"
            accept=".ttf,.otf,font/ttf,font/otf"
            multiple
            hidden
            onChange={(e) => {
              const files = [...(e.target.files ?? [])];
              e.target.value = '';
              void uploadFiles(files);
            }}
          />
        </div>
      )}
    </div>,
    document.body,
  );
}

function FontRow({
  store,
  family,
  kind,
  root,
  selected,
  busy,
  disabled,
  missing,
  onPick,
  extra,
}: {
  store: EditorStore;
  family: string;
  kind: Kind;
  root: React.RefObject<HTMLDivElement | null>;
  selected: boolean;
  busy: boolean;
  disabled: boolean;
  missing: boolean;
  onPick: () => void;
  extra?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [css, setCss] = useState<string | undefined>(kind === 'builtin' || kind === 'local' ? `"${family}"` : undefined);

  // Load a preview once the row scrolls into view.
  useEffect(() => {
    if (css || missing) return;
    const el = ref.current;
    if (!el) return;
    let cancelled = false;
    const io = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        io.disconnect();
        const load =
          kind === 'google'
            ? googlePreviewFamily(family).then((alias) => `"${alias}"`)
            : store.fonts.preview(family).then((ok) => (ok || store.fonts.isAvailable(family) ? `"${family}"` : undefined));
        load.then((c) => !cancelled && c && setCss(c), () => undefined);
      },
      { root: root.current, rootMargin: '120px' },
    );
    io.observe(el);
    return () => {
      cancelled = true;
      io.disconnect();
    };
  }, [css, family, kind, missing, root, store]);

  const tag = kind === 'google' ? 'Google' : kind === 'local' ? 'Local' : kind === 'builtin' ? 'Built in' : null;
  return (
    <div ref={ref} className={`font-row${selected ? ' selected' : ''}`}>
      <button type="button" className="font-row-main" disabled={disabled} onClick={onPick} title={family}>
        <span className="font-row-check">{busy ? <Loader2 size={12} className="spin" /> : selected ? <Check size={12} /> : null}</span>
        <span className="font-row-name" style={css ? { fontFamily: `${css}, Inter, sans-serif` } : undefined}>
          {family}
        </span>
        {missing && <AlertTriangle size={12} className="font-warn-icon" aria-label="Not available" />}
        {tag && <span className="font-row-tag">{tag}</span>}
      </button>
      {extra}
    </div>
  );
}
