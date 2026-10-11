import { useState } from 'react';
import { UserMinus } from 'lucide-react';
import { api, type Member, type Role, type Workspace } from '../../lib/api';
import { useUser } from '../../lib/auth';
import { useDashboard } from '../../lib/dashboard';
import { pluralize, ROLE_LABELS } from '../../lib/format';
import { useAsync } from '../../lib/useAsync';
import { Avatar } from '../../components/Avatar';
import { useDialogs } from '../../components/Dialogs';
import { ErrorState } from '../../components/EmptyState';
import { SettingsSection } from '../../components/SettingsSection';
import { useToast } from '../../components/Toast';

const ROLES: Role[] = ['owner', 'editor', 'viewer'];
const ROLE_ORDER: Record<Role, number> = { owner: 0, editor: 1, viewer: 2 };

export function MembersSection({ workspace }: { workspace: Workspace }) {
  const user = useUser();
  const isOwner = workspace.role === 'owner';
  const { reloadWorkspaces } = useDashboard();
  const dialogs = useDialogs();
  const toast = useToast();
  const members = useAsync(() => api.workspaces.listMembers(workspace.id), `members:${workspace.id}`);
  const [pending, setPending] = useState<string | null>(null);

  const sorted = members.data
    ? [...members.data].sort(
        (a, b) =>
          Number(b.userId === user.id) - Number(a.userId === user.id) ||
          ROLE_ORDER[a.role] - ROLE_ORDER[b.role] ||
          (a.name || a.email).localeCompare(b.name || b.email),
      )
    : undefined;

  const changeRole = async (member: Member, role: Role) => {
    if (role === member.role) return;
    if (role === 'owner') {
      const ok = await dialogs.confirm({
        title: `Make ${member.name || member.email} an owner?`,
        message: 'Owners can rename or delete the workspace, manage members and create invites.',
        confirmLabel: 'Make owner',
      });
      if (!ok) return;
    }
    setPending(member.userId);
    try {
      await api.workspaces.updateMember(workspace.id, member.userId, role);
      members.setData((list) => (list ?? []).map((m) => (m.userId === member.userId ? { ...m, role } : m)));
      toast.success(`${member.name || member.email} is now ${ROLE_LABELS[role].toLowerCase()}`);
    } catch (err) {
      toast.error(err);
    } finally {
      setPending(null);
    }
  };

  const remove = async (member: Member) => {
    const ok = await dialogs.confirm({
      title: `Remove ${member.name || member.email}?`,
      message: `They will lose access to every project and file in ${workspace.name}.`,
      confirmLabel: 'Remove member',
      danger: true,
    });
    if (!ok) return;
    setPending(member.userId);
    try {
      await api.workspaces.removeMember(workspace.id, member.userId);
      members.setData((list) => (list ?? []).filter((m) => m.userId !== member.userId));
      void reloadWorkspaces();
      toast.success(`Removed ${member.name || member.email}`);
    } catch (err) {
      toast.error(err);
    } finally {
      setPending(null);
    }
  };

  return (
    <SettingsSection
      title="Members"
      description="Owners manage the workspace, editors can create and edit files, viewers can only look."
      aside={sorted && <span className="text-muted">{pluralize(sorted.length, 'member')}</span>}
    >
      {!sorted ? (
        members.error ? (
          <ErrorState error={members.error} onRetry={() => void members.reload()} title="Couldn't load members" />
        ) : (
          <ul className="list">
            {[0, 1, 2].map((i) => (
              <li key={i} className="list-row">
                <span className="skeleton skeleton-circle" />
                <span className="list-row-main">
                  <span className="skeleton skeleton-text" style={{ width: 140 }} />
                  <span className="skeleton skeleton-text" style={{ width: 200 }} />
                </span>
              </li>
            ))}
          </ul>
        )
      ) : (
        <ul className="list">
          {sorted.map((m) => {
            const isSelf = m.userId === user.id;
            const busy = pending === m.userId;
            return (
              <li key={m.userId} className="list-row">
                <Avatar name={m.name || m.email} color={m.color} size={32} />
                <span className="list-row-main">
                  <span className="list-row-title">
                    {m.name || m.email}
                    {isSelf && <span className="badge">You</span>}
                  </span>
                  <span className="list-row-sub">{m.email}</span>
                </span>
                <span className="list-row-actions">
                  {isOwner && !isSelf ? (
                    <>
                      <select
                        className="select select-sm"
                        value={m.role}
                        disabled={busy}
                        aria-label={`Role for ${m.name || m.email}`}
                        onChange={(e) => void changeRole(m, e.target.value as Role)}
                      >
                        {ROLES.map((r) => (
                          <option key={r} value={r}>
                            {ROLE_LABELS[r]}
                          </option>
                        ))}
                      </select>
                      <button
                        type="button"
                        className="btn btn-ghost btn-icon btn-sm"
                        aria-label={`Remove ${m.name || m.email}`}
                        title="Remove from workspace"
                        disabled={busy}
                        onClick={() => void remove(m)}
                      >
                        <UserMinus size={16} />
                      </button>
                    </>
                  ) : (
                    <span className={`badge${m.role === 'owner' ? ' badge-accent' : ''}`}>{ROLE_LABELS[m.role]}</span>
                  )}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </SettingsSection>
  );
}
