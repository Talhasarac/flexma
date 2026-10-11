import { useCallback, useEffect, useState } from 'react';
import { Navigate } from 'react-router-dom';
import { KeyRound, ShieldCheck, ShieldOff, Trash2 } from 'lucide-react';
import { api, type AdminUser, type InstanceSettings } from '../../lib/api';
import { useUser } from '../../lib/auth';
import { useDocumentTitle } from '../../lib/useDocumentTitle';
import { formatRelativeTime } from '../../lib/format';
import { Avatar } from '../../components/Avatar';
import { CopyField } from '../../components/CopyField';
import { Modal } from '../../components/Modal';
import { SettingsSection } from '../../components/SettingsSection';
import { useDialogs } from '../../components/Dialogs';
import { useToast } from '../../components/Toast';
import { PageHeader } from '../../components/dashboard/PageHeader';

/** Instance administration: users and sign-up policy (admins only). */
export default function AdminPage() {
  const me = useUser();
  useDocumentTitle('Admin');
  const toast = useToast();
  const dialogs = useDialogs();
  const [users, setUsers] = useState<AdminUser[] | null>(null);
  const [settings, setSettings] = useState<InstanceSettings | null>(null);
  const [tempPassword, setTempPassword] = useState<{ name: string; password: string } | null>(null);

  const load = useCallback(() => {
    api.admin.users().then(setUsers).catch((e: unknown) => toast.error(e));
    api.admin.settings().then(setSettings).catch((e: unknown) => toast.error(e));
  }, [toast]);

  useEffect(() => {
    if (me.isAdmin) load();
  }, [me.isAdmin, load]);

  if (!me.isAdmin) return <Navigate to="/" replace />;

  const act = async (f: () => Promise<unknown>, ok?: string) => {
    try {
      await f();
      if (ok) toast.success(ok);
      load();
    } catch (e) {
      toast.error(e);
    }
  };

  return (
    <div className="page page-narrow">
      <PageHeader title="Admin" subtitle="Manage who can use this Flexma server." />
      <div className="stack stack-lg">
        <SettingsSection title="Sign-up" description="When closed, people can only join with an invite link from a workspace owner.">
          {settings ? (
            <label className="admin-toggle">
              <input
                type="checkbox"
                checked={settings.allowSignup}
                onChange={(e) => void act(() => api.admin.updateSettings({ allowSignup: e.target.checked }), 'Sign-up policy updated')}
              />
              <span>
                <b>Allow anyone to create an account</b>
                <span className="text-muted"> — {settings.allowSignup ? 'open sign-up' : 'invite-only'}</span>
              </span>
            </label>
          ) : (
            <span className="skeleton skeleton-text" />
          )}
        </SettingsSection>

        <SettingsSection title="Users" description="Everyone with an account on this server." aside={users ? <span className="text-muted">{users.length}</span> : null}>
          {!users ? (
            <span className="skeleton skeleton-block" />
          ) : (
            <ul className="admin-users">
              {users.map((u) => (
                <li key={u.id}>
                  <Avatar name={u.name} color={u.color} size={30} />
                  <div className="admin-user-text">
                    <div>
                      <b>{u.name}</b> {u.isAdmin && <span className="badge">Admin</span>} {u.id === me.id && <span className="badge">You</span>}
                    </div>
                    <div className="text-muted text-sm">
                      {u.email} · joined {formatRelativeTime(u.createdAt)} · {u.workspaceCount} workspace{u.workspaceCount === 1 ? '' : 's'}
                      {u.lastSignIn ? ` · last sign-in ${formatRelativeTime(u.lastSignIn)}` : ''}
                    </div>
                  </div>
                  <div className="admin-user-actions">
                    <button
                      type="button"
                      className="btn btn-ghost btn-icon btn-sm"
                      title={u.isAdmin ? 'Remove admin rights' : 'Make admin'}
                      disabled={u.id === me.id}
                      onClick={() => void act(() => api.admin.updateUser(u.id, { isAdmin: !u.isAdmin }))}
                    >
                      {u.isAdmin ? <ShieldOff size={15} /> : <ShieldCheck size={15} />}
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost btn-icon btn-sm"
                      title="Set a temporary password"
                      onClick={async () => {
                        const ok = await dialogs.confirm({
                          title: `Reset ${u.name}'s password?`,
                          message: 'They will be signed out everywhere and need the temporary password to sign in.',
                          confirmLabel: 'Reset password',
                        });
                        if (!ok) return;
                        try {
                          const r = await api.admin.resetPassword(u.id);
                          setTempPassword({ name: u.name, password: r.password });
                        } catch (e) {
                          toast.error(e);
                        }
                      }}
                    >
                      <KeyRound size={15} />
                    </button>
                    <button
                      type="button"
                      className="btn btn-ghost btn-icon btn-sm"
                      title="Delete user"
                      disabled={u.id === me.id}
                      onClick={async () => {
                        const ok = await dialogs.confirm({
                          title: `Delete ${u.name}?`,
                          message: 'Their account and personal workspaces are deleted. Shared workspaces keep their files.',
                          danger: true,
                          confirmLabel: 'Delete user',
                          requireText: u.email,
                        });
                        if (ok) void act(() => api.admin.deleteUser(u.id), 'User deleted');
                      }}
                    >
                      <Trash2 size={15} />
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </SettingsSection>
      </div>
      <Modal
        open={tempPassword !== null}
        onClose={() => setTempPassword(null)}
        title="Temporary password"
        description={tempPassword ? `Give this to ${tempPassword.name}. They can change it under Profile settings.` : undefined}
        size="sm"
      >
        {tempPassword && <CopyField value={tempPassword.password} toastMessage="Password copied" />}
      </Modal>
    </div>
  );
}
