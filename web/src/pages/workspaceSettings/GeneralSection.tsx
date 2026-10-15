import { useState, type FormEvent } from 'react';
import { api, type Workspace } from '../../lib/api';
import { useDashboard } from '../../lib/dashboard';
import { colorFromString, formatDateTime, ROLE_LABELS } from '../../lib/format';
import { Avatar } from '../../components/Avatar';
import { SettingsSection } from '../../components/SettingsSection';
import { Spinner } from '../../components/StatusScreens';
import { useToast } from '../../components/Toast';

export function GeneralSection({ workspace }: { workspace: Workspace }) {
  const isOwner = workspace.role === 'owner';
  const { reloadWorkspaces } = useDashboard();
  const toast = useToast();
  const [name, setName] = useState(workspace.name);
  const [saving, setSaving] = useState(false);

  // Keep the field in sync when switching workspaces.
  const [syncedId, setSyncedId] = useState(workspace.id);
  if (syncedId !== workspace.id) {
    setSyncedId(workspace.id);
    setName(workspace.name);
  }

  const [savingPaint, setSavingPaint] = useState(false);
  const setPaintTools = async (on: boolean) => {
    setSavingPaint(true);
    try {
      await api.workspaces.update(workspace.id, { paintTools: on });
      await reloadWorkspaces();
      toast.success(on ? 'Painting tools turned on' : 'Painting tools turned off');
    } catch (err) {
      toast.error(err);
    } finally {
      setSavingPaint(false);
    }
  };

  const trimmed = name.trim();
  const dirty = trimmed !== workspace.name && trimmed.length > 0;

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!dirty) return;
    setSaving(true);
    try {
      await api.workspaces.update(workspace.id, { name: trimmed });
      await reloadWorkspaces();
      toast.success('Workspace renamed');
    } catch (err) {
      toast.error(err);
    } finally {
      setSaving(false);
    }
  };

  return (
    <SettingsSection title="General" description="Basic information about this workspace.">
      <form className="settings-form" onSubmit={onSubmit}>
        <div className="row row-top">
          <Avatar name={trimmed || workspace.name} color={colorFromString(workspace.id)} size={40} square />
          <label className="field grow">
            <span className="field-label">Workspace name</span>
            {isOwner ? (
              <div className="input-row">
                <input
                  className="input"
                  value={name}
                  maxLength={80}
                  onChange={(e) => setName(e.target.value)}
                  required
                />
                <button type="submit" className="btn btn-primary" disabled={!dirty || saving}>
                  {saving && <Spinner size={14} label="Saving" />}
                  Save
                </button>
              </div>
            ) : (
              <input className="input" value={workspace.name} readOnly />
            )}
            {!isOwner && <span className="field-hint">Only owners can rename the workspace.</span>}
          </label>
        </div>
        <label className="admin-toggle" title={isOwner ? undefined : 'Only owners can change this.'}>
          <input type="checkbox" checked={workspace.paintTools} disabled={!isOwner || savingPaint} onChange={(e) => void setPaintTools(e.target.checked)} />
          <span>
            <b>Painting tools</b>
            <span className="text-muted"> — brush and eraser on paint layers, painted together live. Files open again to pick up the change.</span>
          </span>
        </label>
        <dl className="meta-list">
          <div>
            <dt>Your role</dt>
            <dd>{ROLE_LABELS[workspace.role]}</dd>
          </div>
          <div>
            <dt>Created</dt>
            <dd>{formatDateTime(workspace.createdAt)}</dd>
          </div>
        </dl>
      </form>
    </SettingsSection>
  );
}
