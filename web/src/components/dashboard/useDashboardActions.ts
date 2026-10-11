import { useCallback, useMemo } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { api, type FileInfo, type Project } from '../../lib/api';
import { useDashboard } from '../../lib/dashboard';
import { pluralize } from '../../lib/format';
import { useDialogs } from '../Dialogs';
import { useToast } from '../Toast';

const DRAFTS = 'Drafts';
const UNTITLED = 'Untitled';

/** Workspace / project / file creation flows shared by the sidebar and pages. */
export function useDashboardActions() {
  const navigate = useNavigate();
  const location = useLocation();
  const toast = useToast();
  const dialogs = useDialogs();
  const { workspace, projects, reloadProjects, reloadWorkspaces, notifyFilesChanged } = useDashboard();

  const createWorkspace = useCallback(async () => {
    const name = await dialogs.prompt({
      title: 'New workspace',
      message: 'Workspaces are shared spaces for a team: invite people and they see every project inside.',
      label: 'Workspace name',
      placeholder: 'e.g. Acme Design',
      confirmLabel: 'Create workspace',
      maxLength: 80,
    });
    if (!name) return;
    try {
      const ws = await api.workspaces.create(name);
      await reloadWorkspaces();
      navigate(`/w/${ws.id}`);
      toast.success(`Created ${ws.name}`);
    } catch (err) {
      toast.error(err);
    }
  }, [dialogs, navigate, reloadWorkspaces, toast]);

  const createProject = useCallback(async () => {
    if (!workspace) return;
    const name = await dialogs.prompt({
      title: 'New project',
      label: 'Project name',
      placeholder: 'e.g. Marketing site',
      confirmLabel: 'Create project',
      maxLength: 120,
    });
    if (!name) return;
    try {
      const project = await api.projects.create(workspace.id, name);
      await reloadProjects();
      navigate(`/w/${workspace.id}/p/${project.id}`);
    } catch (err) {
      toast.error(err);
    }
  }, [dialogs, navigate, reloadProjects, toast, workspace]);

  const renameProject = useCallback(
    async (project: Project) => {
      const name = await dialogs.prompt({
        title: 'Rename project',
        label: 'Project name',
        defaultValue: project.name,
        confirmLabel: 'Rename',
        maxLength: 120,
      });
      if (!name || name === project.name) return;
      try {
        await api.projects.update(project.id, { name });
        notifyFilesChanged(); // also reloads projects
      } catch (err) {
        toast.error(err);
      }
    },
    [dialogs, notifyFilesChanged, toast],
  );

  const deleteProject = useCallback(
    async (project: Project) => {
      const ok = await dialogs.confirm({
        title: `Delete “${project.name}”?`,
        message:
          project.fileCount > 0
            ? `This permanently deletes the project and its ${pluralize(project.fileCount, 'file')}. This can't be undone.`
            : "This permanently deletes the project. This can't be undone.",
        confirmLabel: 'Delete project',
        danger: true,
      });
      if (!ok) return;
      try {
        await api.projects.delete(project.id);
        toast.success(`Deleted ${project.name}`);
        if (location.pathname.includes(`/p/${project.id}`)) navigate(`/w/${project.workspaceId}`, { replace: true });
        notifyFilesChanged(); // also reloads projects
      } catch (err) {
        toast.error(err);
      }
    },
    [dialogs, location.pathname, navigate, notifyFilesChanged, toast],
  );

  /**
   * Create an "Untitled" design file and open it. Without a project id the
   * file goes into the workspace's "Drafts" project (created on demand).
   */
  const createFile = useCallback(
    async (projectId?: string): Promise<FileInfo | null> => {
      if (!workspace) return null;
      try {
        let target = projectId;
        if (!target) {
          const list = projects ?? (await api.projects.list(workspace.id));
          const drafts = list.find((p) => p.name.trim().toLowerCase() === DRAFTS.toLowerCase());
          target = drafts ? drafts.id : (await api.projects.create(workspace.id, DRAFTS)).id;
        }
        const file = await api.files.create(target, UNTITLED);
        notifyFilesChanged();
        navigate(`/file/${file.id}`);
        return file;
      } catch (err) {
        toast.error(err);
        return null;
      }
    },
    [navigate, notifyFilesChanged, projects, toast, workspace],
  );

  return useMemo(
    () => ({ createWorkspace, createProject, renameProject, deleteProject, createFile }),
    [createWorkspace, createProject, renameProject, deleteProject, createFile],
  );
}
