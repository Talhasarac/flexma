import { useState, type FormEvent } from 'react';
import { Link2, Mail, Trash } from 'lucide-react';
import { api, type Invite, type Role, type Workspace } from '../../lib/api';
import { formatRelativeTime, ROLE_LABELS } from '../../lib/format';
import { useAsync } from '../../lib/useAsync';
import { CopyButton, CopyField } from '../../components/CopyField';
import { useDialogs } from '../../components/Dialogs';
import { ErrorState } from '../../components/EmptyState';
import { SettingsSection } from '../../components/SettingsSection';
import { Spinner } from '../../components/StatusScreens';
import { TimeAgo } from '../../components/TimeAgo';
import { useToast } from '../../components/Toast';

/** Owner-only: create invite links and manage pending invites. */
export function InvitesSection({ workspace }: { workspace: Workspace }) {
  const toast = useToast();
  const dialogs = useDialogs();
  const invites = useAsync(() => api.workspaces.listInvites(workspace.id), `invites:${workspace.id}`);
  const [role, setRole] = useState<Role>('editor');
  const [email, setEmail] = useState('');
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<Invite | null>(null);

  const onCreate = async (e: FormEvent) => {
    e.preventDefault();
    setCreating(true);
    try {
      const invite = await api.workspaces.createInvite(workspace.id, {
        role,
        email: email.trim() || undefined,
      });
      setCreated(invite);
      setEmail('');
      invites.setData((list) => [invite, ...(list ?? []).filter((i) => i.id !== invite.id)]);
    } catch (err) {
      toast.error(err);
    } finally {
      setCreating(false);
    }
  };

  const revoke = async (invite: Invite) => {
    const ok = await dialogs.confirm({
      title: 'Revoke invite?',
      message: 'The link will stop working immediately. People who already joined keep their access.',
      confirmLabel: 'Revoke',
      danger: true,
    });
    if (!ok) return;
    try {
      await api.invites.delete(invite.id);
      invites.setData((list) => (list ?? []).filter((i) => i.id !== invite.id));
      if (created?.id === invite.id) setCreated(null);
      toast.success('Invite revoked');
    } catch (err) {
      toast.error(err);
    }
  };

  const pending = invites.data ?? [];

  return (
    <SettingsSection
      title="Invite people"
      description="Create a link and share it with your team. Anyone who opens it can join this workspace with the role you pick."
    >
      <form className="invite-form" onSubmit={onCreate}>
        <label className="field grow">
          <span className="field-label">Email (optional)</span>
          <input
            className="input"
            type="email"
            placeholder="teammate@example.com"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            autoComplete="off"
          />
        </label>
        <label className="field">
          <span className="field-label">Role</span>
          <select className="select" value={role} onChange={(e) => setRole(e.target.value as Role)}>
            <option value="editor">{ROLE_LABELS.editor}</option>
            <option value="viewer">{ROLE_LABELS.viewer}</option>
            <option value="owner">{ROLE_LABELS.owner}</option>
          </select>
        </label>
        <button type="submit" className="btn btn-primary invite-form-submit" disabled={creating}>
          {creating ? <Spinner size={14} label="Creating" /> : <Link2 size={16} aria-hidden="true" />}
          Create invite link
        </button>
      </form>

      {created && (
        <div className="callout callout-success">
          <p className="callout-title">
            Invite link ready{created.email ? ` for ${created.email}` : ''}. Share it with your teammate:
          </p>
          <CopyField value={created.url} toastMessage="Invite link copied" />
        </div>
      )}

      <div className="subsection">
        <h3 className="subsection-title">Pending invites</h3>
        {!invites.data && invites.error ? (
          <ErrorState error={invites.error} onRetry={() => void invites.reload()} title="Couldn't load invites" />
        ) : !invites.data ? (
          <div className="list-placeholder">
            <span className="skeleton skeleton-text" style={{ width: '60%' }} />
          </div>
        ) : pending.length === 0 ? (
          <p className="text-muted">No pending invites.</p>
        ) : (
          <ul className="list">
            {pending.map((inv) => (
              <li key={inv.id} className="list-row">
                <span className="list-row-icon" aria-hidden="true">
                  {inv.email ? <Mail size={16} /> : <Link2 size={16} />}
                </span>
                <span className="list-row-main">
                  <span className="list-row-title">
                    {inv.email || 'Anyone with the link'}
                    <span className="badge">{ROLE_LABELS[inv.role]}</span>
                  </span>
                  <span className="list-row-sub">
                    <TimeAgo date={inv.createdAt} prefix="Created" />
                    {inv.expiresAt && <> · {expiryLabel(inv.expiresAt)}</>}
                  </span>
                </span>
                <span className="list-row-actions">
                  <CopyButton text={inv.url} label="Copy link" toastMessage="Invite link copied" iconOnly />
                  <button
                    type="button"
                    className="btn btn-ghost btn-icon btn-sm"
                    aria-label="Revoke invite"
                    title="Revoke invite"
                    onClick={() => void revoke(inv)}
                  >
                    <Trash size={16} />
                  </button>
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </SettingsSection>
  );
}

function expiryLabel(expiresAt: string): string {
  const t = Date.parse(expiresAt);
  if (Number.isNaN(t)) return '';
  if (t <= Date.now()) return 'Expired';
  const days = Math.ceil((t - Date.now()) / 86_400_000);
  if (days <= 1) {
    const hours = Math.max(1, Math.ceil((t - Date.now()) / 3_600_000));
    return `Expires in ${hours}h`;
  }
  if (days <= 30) return `Expires in ${days} days`;
  return `Expires ${formatRelativeTime(expiresAt)}`;
}
