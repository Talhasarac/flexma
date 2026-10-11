// Fonts this file uses that can't be shown here: add them from Google Fonts,
// or replace them everywhere in the file.
import { useEffect, useState } from 'react';
import { AlertTriangle, Loader2 } from 'lucide-react';
import { api, type GoogleFontFamily } from '../lib/api';
import { findGoogleFamily, googleCatalogue } from '../lib/fonts';
import { Modal } from '../components/Modal';
import { useToast } from '../components/Toast';
import { FontPicker } from './FontPicker';
import { useFontState, useUi, type EditorStore } from './store';

export function MissingFontsButton({ store }: { store: EditorStore }) {
  const { missing } = useFontState(store);
  const [open, setOpen] = useState(false);
  if (missing.length === 0 && !open) return null;
  return (
    <>
      {missing.length > 0 && (
      <button
        type="button"
        className="btn btn-sm tb-missing-fonts"
        onClick={() => setOpen(true)}
        title="Some fonts in this file aren't available, so a fallback is shown"
      >
        <AlertTriangle size={13} /> {missing.length} missing {missing.length === 1 ? 'font' : 'fonts'}
      </button>
      )}
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        title="Missing fonts"
        description="These fonts are used in this file but aren't in the workspace or on this computer, so a fallback font is shown in their place."
        size="md"
      >
        <MissingFontsList store={store} />
      </Modal>
    </>
  );
}

function MissingFontsList({ store }: { store: EditorStore }) {
  const { missing } = useFontState(store);
  const ui = useUi(store);
  const toast = useToast();
  const [google, setGoogle] = useState<GoogleFontFamily[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const canFix = ui.canEdit && !!store.workspaceId;

  useEffect(() => {
    googleCatalogue().then(setGoogle, () => setGoogle([]));
  }, []);

  if (missing.length === 0) return <p className="text-muted">All fonts in this file are available.</p>;

  const add = async (family: string) => {
    setBusy(family);
    try {
      await api.fonts.addGoogle(store.workspaceId!, family);
      await store.fonts.refresh();
      toast.success(`Added ${family} to the workspace`, { id: 'font-added' });
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(null);
    }
  };

  const replace = (from: string, to: string) => {
    const n = store.engine.replace_font(from, to);
    store.poll();
    toast.success(`Replaced ${from} with ${to} in ${n} ${n === 1 ? 'layer' : 'layers'}`);
  };

  return (
    <ul className="missing-fonts">
      {missing.map((m) => {
        const g = findGoogleFamily(google, m.family);
        return (
          <li key={m.family}>
            <div className="missing-fonts-name">
              <b>{m.family}</b>
              <span className="text-muted">
                {m.count} text {m.count === 1 ? 'layer' : 'layers'}
              </span>
            </div>
            {canFix ? (
              <div className="missing-fonts-actions">
                {g && (
                  <button type="button" className="btn btn-sm" disabled={!!busy} onClick={() => void add(g.family)}>
                    {busy === g.family && <Loader2 size={13} className="spin" />} Add from Google Fonts
                  </button>
                )}
                <FontPicker
                  store={store}
                  compact
                  value=""
                  placeholder="Replace with…"
                  styles={m.styles}
                  onPick={(to) => replace(m.family, to)}
                />
              </div>
            ) : (
              <span className="text-muted">Ask an editor to add it.</span>
            )}
          </li>
        );
      })}
    </ul>
  );
}
