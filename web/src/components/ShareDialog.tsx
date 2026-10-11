// "Share" dialog: the member link plus the file's public view-only link.
import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { Globe, RefreshCw, Users } from 'lucide-react';
import { api, type FileInfo, type ShareLink, type ShareUpdate } from '../lib/api';
import { canEdit, formatDateTime } from '../lib/format';
import { CopyButton, CopyField } from './CopyField';
import { useDialogs } from './Dialogs';
import { Modal } from './Modal';
import { useToast } from './Toast';

type SharedFile = Pick<FileInfo, 'id' | 'name' | 'role' | 'workspaceId'>;

const EXPIRY_CHOICES = [
  { days: 0, label: 'Never expires' },
  { days: 1, label: 'Expires in 1 day' },
  { days: 7, label: 'Expires in 7 days' },
  { days: 30, label: 'Expires in 30 days' },
];

export function ShareDialog({ open, onClose, file }: { open: boolean; onClose: () => void; file: SharedFile }) {
  return (
    <Modal open={open} onClose={onClose} title={`Share “${file.name}”`} size="md">
      <ShareBody file={file} />
    </Modal>
  );
}

function ShareBody({ file }: { file: SharedFile }) {
  const toast = useToast();
  const dialogs = useDialogs();
  const [link, setLink] = useState<ShareLink | null>(null);
  const [busy, setBusy] = useState(false);
  const editable = canEdit(file.role);

  useEffect(() => {
    let cancelled = false;
    api.share
      .get(file.id)
      .then((l) => !cancelled && setLink(l))
      .catch((err: unknown) => {
        if (cancelled) return;
        toast.error(err);
        setLink({ enabled: false });
      });
    return () => {
      cancelled = true;
    };
  }, [file.id, toast]);

  const run = async (body: ShareUpdate, done: string) => {
    setBusy(true);
    try {
      setLink(await api.share.update(file.id, body));
      toast.success(done, { id: 'share' });
    } catch (err) {
      toast.error(err);
    } finally {
      setBusy(false);
    }
  };

  const reset = async () => {
    const ok = await dialogs.confirm({
      title: 'Reset the public link?',
      message: 'The current link stops working right away and anyone viewing through it is disconnected.',
      confirmLabel: 'Reset link',
      danger: true,
    });
    if (ok) await run({ enabled: true, regenerate: true }, 'New link created. The old one no longer works.');
  };

  const memberUrl = `${window.location.origin}/file/${file.id}`;
  const on = !!link?.enabled;

  return (
    <div className="share-dialog">
      <section className="share-row">
        <span className="share-icon" aria-hidden="true">
          <Users size={16} />
        </span>
        <div className="share-text">
          <div className="share-title">Workspace members</div>
          <div className="share-sub">
            People in the workspace open it with their own role.{' '}
            <Link to={`/w/${file.workspaceId}/settings`}>Invite people</Link>
          </div>
        </div>
        <CopyButton text={memberUrl} label="Copy link" toastMessage="Link copied" />
      </section>

      <section className="share-row">
        <span className={`share-icon${on ? ' share-icon-on' : ''}`} aria-hidden="true">
          <Globe size={16} />
        </span>
        <div className="share-text">
          <div className="share-title" id="share-public-label">
            Public link
          </div>
          <div className="share-sub">
            {on
              ? "Anyone with the link can view and export, without signing in. They can't edit or see comments."
              : 'Off. Turn it on to share a view-only link with anyone.'}
          </div>
        </div>
        <label className="switch" title={editable ? undefined : 'Only editors can change link sharing'}>
          <input
            type="checkbox"
            role="switch"
            aria-labelledby="share-public-label"
            checked={on}
            disabled={!editable || busy || !link}
            onChange={(ev) =>
              void run(
                ev.target.checked ? { enabled: true } : { enabled: false },
                ev.target.checked ? 'Public link is on' : 'Public link turned off. Anyone using it was disconnected.',
              )
            }
          />
          <span className="switch-track" />
        </label>
      </section>

      {on && link?.url && (
        <div className="share-link">
          <CopyField value={link.url} toastMessage="Public link copied" />
          <div className="share-options">
            <select
              className="select select-sm"
              aria-label="Link expiry"
              value={link.expiresAt ? 'current' : '0'}
              disabled={!editable || busy}
              onChange={(ev) => {
                if (ev.target.value === 'current') return;
                const days = Number(ev.target.value);
                void run({ enabled: true, expiresInDays: days }, days ? `Link expires in ${days} ${days === 1 ? 'day' : 'days'}` : 'Link never expires');
              }}
            >
              {link.expiresAt && <option value="current">Expires {formatDateTime(link.expiresAt)}</option>}
              {EXPIRY_CHOICES.map((c) => (
                <option key={c.days} value={String(c.days)}>
                  {c.label}
                </option>
              ))}
            </select>
            <button type="button" className="btn btn-sm btn-ghost" disabled={!editable || busy} onClick={() => void reset()}>
              <RefreshCw size={13} /> Reset link
            </button>
          </div>
          {link.createdBy && <p className="share-note">Created by {link.createdBy}</p>}
        </div>
      )}
      {!editable && <p className="share-note">Only editors can change link sharing.</p>}
    </div>
  );
}
