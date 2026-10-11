// Version history: browse automatic snapshots and named versions, preview,
// save a named version, and restore (the current state is kept as a version).
import { useCallback, useEffect, useState } from 'react';
import { History, RotateCcw } from 'lucide-react';
import { Modal } from '../components/Modal';
import { useToast } from '../components/Toast';
import { useDialogs } from '../components/Dialogs';
import { api, type FileVersion } from '../lib/api';
import { formatRelativeTime } from '../lib/format';

interface Props {
  open: boolean;
  onClose: () => void;
  fileId: string;
  canEdit: boolean;
}

export function VersionsDialog({ open, onClose, fileId, canEdit }: Props) {
  const toast = useToast();
  const dialogs = useDialogs();
  const [versions, setVersions] = useState<FileVersion[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    api.files
      .versions(fileId)
      .then((v) => {
        setVersions(v);
        setSelected((s) => s ?? v[0]?.id ?? null);
      })
      .catch((e: unknown) => toast.error(e));
  }, [fileId, toast]);

  useEffect(() => {
    if (open) load();
    else {
      setVersions(null);
      setSelected(null);
    }
  }, [open, load]);

  const saveNamed = async () => {
    const label = await dialogs.prompt({ title: 'Save version', label: 'Name', placeholder: 'e.g. Before the redesign', maxLength: 120 });
    if (!label) return;
    try {
      await api.files.saveVersion(fileId, label);
      toast.success('Version saved');
      load();
    } catch (e) {
      toast.error(e);
    }
  };

  const restore = async (v: FileVersion) => {
    const ok = await dialogs.confirm({
      title: 'Restore this version?',
      message: 'Everyone in the file will see it immediately. The current state is saved to the history first, so you can switch back.',
      confirmLabel: 'Restore',
    });
    if (!ok) return;
    setBusy(true);
    try {
      await api.files.restoreVersion(fileId, v.id);
      onClose();
    } catch (e) {
      toast.error(e);
    } finally {
      setBusy(false);
    }
  };

  const current = versions?.find((v) => v.id === selected) ?? null;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Version history"
      description="Flexma snapshots files automatically while you work. Named versions are kept forever."
      size="lg"
      footer={
        <>
          {canEdit && (
            <button type="button" className="btn" onClick={() => void saveNamed()}>
              <History size={14} /> Save current version
            </button>
          )}
          {canEdit && current && (
            <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void restore(current)}>
              <RotateCcw size={14} /> Restore this version
            </button>
          )}
        </>
      }
    >
      {!versions ? (
        <div className="skeleton skeleton-block" style={{ height: 240 }} />
      ) : versions.length === 0 ? (
        <p className="text-muted">No versions yet. Snapshots appear here as the file is edited.</p>
      ) : (
        <div className="versions">
          <ul className="versions-list">
            {versions.map((v) => (
              <li key={v.id}>
                <button type="button" className={`version-item${v.id === selected ? ' active' : ''}`} onClick={() => setSelected(v.id)}>
                  <span className="version-title">{v.label ?? 'Autosave'}</span>
                  <span className="version-meta">
                    {formatRelativeTime(v.createdAt)}
                    {v.createdBy ? ` · ${v.createdBy}` : ''}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          <div className="version-preview">
            {current && <img src={api.files.versionPreviewUrl(fileId, current.id)} alt="Preview of the selected version" />}
          </div>
        </div>
      )}
    </Modal>
  );
}
