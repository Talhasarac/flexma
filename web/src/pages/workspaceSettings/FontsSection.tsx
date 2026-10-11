// Workspace fonts: everyone in the workspace sees them in every file (and so
// do share-link viewers and exports). Editors upload files or add Google Fonts.
import { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, Plus, Trash, Upload } from 'lucide-react';
import { api, errorMessage, type GoogleFontFamily, type Workspace, type WorkspaceFont } from '../../lib/api';
import { canEdit, formatRelativeTime } from '../../lib/format';
import {
  closestFace,
  fontFaceFor,
  formatBytes,
  googleCatalogue,
  googlePreviewFamily,
  groupByFamily,
  sameFamily,
  styleName,
} from '../../lib/fonts';
import { useAsync } from '../../lib/useAsync';
import { useDialogs } from '../../components/Dialogs';
import { ErrorState } from '../../components/EmptyState';
import { SettingsSection } from '../../components/SettingsSection';
import { useToast } from '../../components/Toast';

export function FontsSection({ workspace }: { workspace: Workspace }) {
  const editable = canEdit(workspace.role);
  const toast = useToast();
  const dialogs = useDialogs();
  const fonts = useAsync(() => api.fonts.list(workspace.id), `fonts:${workspace.id}`);
  const [busy, setBusy] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const families = useMemo(() => groupByFamily(fonts.data ?? []), [fonts.data]);

  const upload = async (files: File[]) => {
    if (files.length === 0) return;
    setBusy('upload');
    const added = new Set<string>();
    for (const f of files) {
      try {
        added.add((await api.fonts.upload(workspace.id, f)).family);
      } catch (err) {
        toast.error(`${f.name}: ${errorMessage(err)}`);
      }
    }
    setBusy(null);
    if (added.size) {
      toast.success(`Added ${[...added].join(', ')}`);
      fonts.reload();
    }
  };

  const remove = async (family: string, faces: WorkspaceFont[]) => {
    const ok = await dialogs.confirm({
      title: `Remove ${family}?`,
      message: `Its ${faces.length} ${faces.length === 1 ? 'style' : 'styles'} will be removed from the workspace. Text using ${family} will show a fallback font until it's added again.`,
      confirmLabel: 'Remove font',
      danger: true,
    });
    if (!ok) return;
    setBusy(family);
    try {
      await api.fonts.deleteFamily(workspace.id, family);
      fonts.reload();
      toast.success(`Removed ${family}`);
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(null);
    }
  };

  return (
    <SettingsSection
      title="Fonts"
      description="Fonts here work in every file in the workspace, for everyone, including people viewing share links and in exports. Only upload fonts you're licensed to use on the web."
      aside={
        editable && (
          <>
            <button type="button" className="btn btn-sm" disabled={!!busy} onClick={() => fileRef.current?.click()}>
              {busy === 'upload' ? <Loader2 size={14} className="spin" /> : <Upload size={14} />} Upload fonts
            </button>
            <input
              ref={fileRef}
              type="file"
              accept=".ttf,.otf,font/ttf,font/otf"
              multiple
              hidden
              onChange={(e) => {
                const files = [...(e.target.files ?? [])];
                e.target.value = '';
                void upload(files);
              }}
            />
          </>
        )
      }
    >
      {editable && (
        <GoogleFontAdder
          workspace={workspace}
          existing={[...families.keys()]}
          onAdded={() => fonts.reload()}
        />
      )}
      {fonts.error && !fonts.data ? (
        <ErrorState error={fonts.error} onRetry={fonts.reload} title="Couldn't load fonts" />
      ) : !fonts.data ? (
        <p className="text-muted">Loading…</p>
      ) : families.size === 0 ? (
        <p className="text-muted">
          No fonts yet. Inter is always available.{editable ? ' Upload .ttf or .otf files, or add families from Google Fonts.' : ''}
        </p>
      ) : (
        <ul className="ws-fonts">
          {[...families.entries()].map(([family, faces]) => (
            <FontFamilyRow
              key={family}
              family={family}
              faces={faces}
              busy={busy === family}
              onRemove={editable ? () => void remove(family, faces) : undefined}
            />
          ))}
        </ul>
      )}
    </SettingsSection>
  );
}

function FontFamilyRow({ family, faces, busy, onRemove }: { family: string; faces: WorkspaceFont[]; busy: boolean; onRemove?: () => void }) {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const regular = closestFace(faces, 400, false);
    if (!regular) return;
    let live = true;
    fontFaceFor(regular, api.fonts.fileUrl(regular.id))
      .load()
      .then(() => live && setReady(true), () => undefined);
    return () => {
      live = false;
    };
  }, [faces]);
  const size = faces.reduce((n, f) => n + f.size, 0);
  const source = faces.every((f) => f.source === 'google') ? 'Google Fonts' : faces.every((f) => f.source === 'upload') ? 'Uploaded' : 'Mixed';
  const styles = faces.map((f) => styleName(f.weight, f.italic));
  const newest = faces.reduce((a, b) => (a.createdAt > b.createdAt ? a : b));
  return (
    <li>
      <div className="ws-font-main">
        <div className="ws-font-preview" style={ready ? { fontFamily: `"${family}", Inter, sans-serif` } : undefined}>
          {family}
        </div>
        <div className="ws-font-meta">
          <span className="ws-font-badge">{source}</span>
          {faces.length <= 4 ? styles.join(', ') : `${faces.length} styles`} · {formatBytes(size)}
          {newest.createdBy && ` · added by ${newest.createdBy} ${formatRelativeTime(newest.createdAt)}`}
        </div>
      </div>
      {onRemove && (
        <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label={`Remove ${family}`} title="Remove" disabled={busy} onClick={onRemove}>
          {busy ? <Loader2 size={14} className="spin" /> : <Trash size={14} />}
        </button>
      )}
    </li>
  );
}

function GoogleFontAdder({ workspace, existing, onAdded }: { workspace: Workspace; existing: string[]; onAdded: () => void }) {
  const toast = useToast();
  const [catalogue, setCatalogue] = useState<GoogleFontFamily[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [adding, setAdding] = useState<string | null>(null);

  useEffect(() => {
    googleCatalogue().then(setCatalogue, (err: unknown) => setError(errorMessage(err, 'Google Fonts is unavailable')));
  }, []);

  const q = query.trim().toLowerCase();
  const results = q && catalogue ? catalogue.filter((f) => f.family.toLowerCase().includes(q)).slice(0, 6) : [];

  const add = async (family: string) => {
    setAdding(family);
    try {
      const res = await api.fonts.addGoogle(workspace.id, family);
      toast.success(`Added ${res.family} (${res.styles.length} ${res.styles.length === 1 ? 'style' : 'styles'})`);
      setQuery('');
      onAdded();
    } catch (err) {
      toast.error(err);
    } finally {
      setAdding(null);
    }
  };

  return (
    <div className="ws-google">
      <input
        className="input"
        value={query}
        placeholder={error ?? `Add from Google Fonts: search ${catalogue ? catalogue.length.toLocaleString() : ''} families`}
        disabled={!!error}
        onChange={(e) => setQuery(e.target.value)}
        aria-label="Search Google Fonts"
      />
      {results.length > 0 && (
        <ul className="ws-google-results">
          {results.map((f) => {
            const have = existing.some((e) => sameFamily(e, f.family));
            return (
              <GoogleResult
                key={f.family}
                font={f}
                have={have}
                adding={adding === f.family}
                disabled={!!adding}
                onAdd={() => void add(f.family)}
              />
            );
          })}
        </ul>
      )}
    </div>
  );
}

function GoogleResult({ font, have, adding, disabled, onAdd }: { font: GoogleFontFamily; have: boolean; adding: boolean; disabled: boolean; onAdd: () => void }) {
  const [css, setCss] = useState<string | undefined>();
  useEffect(() => {
    let live = true;
    googlePreviewFamily(font.family).then((alias) => live && setCss(`"${alias}"`), () => undefined);
    return () => {
      live = false;
    };
  }, [font.family]);
  return (
    <li>
      <span className="ws-google-name" style={css ? { fontFamily: `${css}, Inter, sans-serif` } : undefined}>
        {font.family}
      </span>
      <span className="text-muted">
        {font.category} · {font.styles.length} {font.styles.length === 1 ? 'style' : 'styles'}
      </span>
      <button type="button" className="btn btn-sm" disabled={have || disabled} onClick={onAdd}>
        {adding ? <Loader2 size={13} className="spin" /> : have ? null : <Plus size={13} />} {have ? 'Added' : 'Add'}
      </button>
    </li>
  );
}
