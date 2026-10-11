import { useNavigate } from 'react-router-dom';
import { LogOut, Trash } from 'lucide-react';
import { api } from '../../lib/api';
import { useUser } from '../../lib/auth';
import { useWorkspace } from '../../lib/dashboard';
import { LAST_WORKSPACE_KEY, writeStorage } from '../../lib/storage';
import { useDocumentTitle } from '../../lib/useDocumentTitle';
import { useDialogs } from '../../components/Dialogs';
import { SettingsSection } from '../../components/SettingsSection';
import { useToast } from '../../components/Toast';
import { PageHeader } from '../../components/dashboard/PageHeader';
import { FontsSection } from './FontsSection';
import { GeneralSection } from './GeneralSection';
import { InvitesSection } from './InvitesSection';
import { MembersSection } from './MembersSection';

export default function WorkspaceSettingsPage() {
  const workspace = useWorkspace();
  const isOwner = workspace.role === 'owner';
  useDocumentTitle(`Settings · ${workspace.name}`);

  return (
    <div className="page page-narrow">
      <PageHeader title="Workspace settings" subtitle={workspace.name} />
      <div className="stack stack-lg">
        <GeneralSection workspace={workspace} />
        <MembersSection workspace={workspace} />
        {isOwner && <InvitesSection workspace={workspace} />}
        <FontsSection workspace={workspace} />
        <DangerZone />
      </div>
    </div>
  );
}

function DangerZone() {
  const workspace = useWorkspace();
  const user = useUser();
  const dialogs = useDialogs();
  const toast = useToast();
  const navigate = useNavigate();
  const isOwner = workspace.role === 'owner';

  const leave = async () => {
    const ok = await dialogs.confirm({
      title: `Leave ${workspace.name}?`,
      message: "You'll lose access to its projects and files until someone invites you again.",
      confirmLabel: 'Leave workspace',
      danger: true,
    });
    if (!ok) return;
    try {
      await api.workspaces.removeMember(workspace.id, user.id);
      writeStorage(LAST_WORKSPACE_KEY, null);
      // "/" re-resolves the workspace list from scratch.
      navigate('/', { replace: true });
      toast.success(`You left ${workspace.name}`);
    } catch (err) {
      toast.error(err);
    }
  };

  const remove = async () => {
    const ok = await dialogs.confirm({
      title: `Delete ${workspace.name}?`,
      message:
        'This permanently deletes the workspace with all of its projects and files for every member. This cannot be undone.',
      confirmLabel: 'Delete workspace',
      danger: true,
      requireText: workspace.name,
    });
    if (!ok) return;
    try {
      await api.workspaces.delete(workspace.id);
      writeStorage(LAST_WORKSPACE_KEY, null);
      navigate('/', { replace: true });
      toast.success(`Deleted ${workspace.name}`);
    } catch (err) {
      toast.error(err);
    }
  };

  return (
    <SettingsSection title="Danger zone" tone="danger">
      <div className="danger-row">
        <div>
          <div className="danger-row-title">Leave workspace</div>
          <div className="text-muted">Remove yourself from {workspace.name}.</div>
        </div>
        <button type="button" className="btn" onClick={() => void leave()}>
          <LogOut size={16} aria-hidden="true" />
          Leave
        </button>
      </div>
      {isOwner && (
        <div className="danger-row">
          <div>
            <div className="danger-row-title">Delete workspace</div>
            <div className="text-muted">Permanently delete this workspace, its projects and files.</div>
          </div>
          <button type="button" className="btn btn-danger" onClick={() => void remove()}>
            <Trash size={16} aria-hidden="true" />
            Delete
          </button>
        </div>
      )}
    </SettingsSection>
  );
}
