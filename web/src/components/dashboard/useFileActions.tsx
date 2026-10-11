import { useCallback, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { CopyPlus, ExternalLink, FolderInput, Link2, PencilLine, Share2, SquareArrowOutUpRight, Trash } from 'lucide-react';
import { api, type FileInfo } from '../../lib/api';
import { copyText } from '../../lib/clipboard';
import { useDashboard } from '../../lib/dashboard';
import { canEdit } from '../../lib/format';
import { useDialogs } from '../Dialogs';
import type { MenuEntry } from '../Menu';
import { ShareDialog } from '../ShareDialog';
import { useToast } from '../Toast';

export function fileUrl(file: Pick<FileInfo, 'id'>): string {
  return `/file/${file.id}`;
}

/** Rename / duplicate / move / share / delete flows for file cards. Render `element`. */
export function useFileActions() {
  const navigate = useNavigate();
  const toast = useToast();
  const dialogs = useDialogs();
  const { projects, notifyFilesChanged } = useDashboard();
  const [sharing, setSharing] = useState<FileInfo | null>(null);

  const open = useCallback((file: FileInfo) => navigate(fileUrl(file)), [navigate]);

  const rename = useCallback(
    async (file: FileInfo) => {
      const name = await dialogs.prompt({
        title: 'Rename file',
        label: 'File name',
        defaultValue: file.name,
        confirmLabel: 'Rename',
        maxLength: 200,
      });
      if (!name || name === file.name) return;
      try {
        await api.files.update(file.id, { name });
        notifyFilesChanged();
      } catch (err) {
        toast.error(err);
      }
    },
    [dialogs, notifyFilesChanged, toast],
  );

  const duplicate = useCallback(
    async (file: FileInfo) => {
      try {
        const copy = await api.files.duplicate(file.id);
        notifyFilesChanged();
        toast.success(`Duplicated as “${copy.name}”`, {
          action: { label: 'Open', onClick: () => navigate(fileUrl(copy)) },
        });
      } catch (err) {
        toast.error(err);
      }
    },
    [navigate, notifyFilesChanged, toast],
  );

  const moveTo = useCallback(
    async (file: Pick<FileInfo, 'id' | 'name' | 'projectId'>, projectId: string) => {
      if (projectId === file.projectId) return;
      try {
        const moved = await api.files.update(file.id, { projectId });
        notifyFilesChanged();
        const target = projects?.find((p) => p.id === projectId)?.name ?? moved.projectName;
        toast.success(`Moved “${file.name}” to ${target}`);
      } catch (err) {
        toast.error(err);
      }
    },
    [notifyFilesChanged, projects, toast],
  );

  const move = useCallback(
    async (file: FileInfo) => {
      const list = [...(projects ?? [])].sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
      );
      if (list.length < 2) {
        toast.info('Create another project first, then move files into it.');
        return;
      }
      const projectId = await dialogs.choose({
        title: `Move “${file.name}”`,
        message: 'Choose a project in this workspace.',
        options: list.map((p) => ({
          value: p.id,
          label: p.name,
          description: p.id === file.projectId ? 'Current project' : undefined,
        })),
        defaultValue: file.projectId,
        confirmLabel: 'Move',
      });
      if (projectId) await moveTo(file, projectId);
    },
    [dialogs, moveTo, projects, toast],
  );

  const copyLink = useCallback(
    async (file: FileInfo) => {
      const ok = await copyText(new URL(fileUrl(file), window.location.origin).toString());
      if (ok) toast.success('Link copied');
      else toast.error('Could not copy the link');
    },
    [toast],
  );

  const remove = useCallback(
    async (file: FileInfo) => {
      const ok = await dialogs.confirm({
        title: `Delete “${file.name}”?`,
        message: "The file will be permanently deleted for everyone in the workspace. This can't be undone.",
        confirmLabel: 'Delete file',
        danger: true,
      });
      if (!ok) return;
      try {
        await api.files.delete(file.id);
        notifyFilesChanged();
        toast.success(`Deleted “${file.name}”`);
      } catch (err) {
        toast.error(err);
      }
    },
    [dialogs, notifyFilesChanged, toast],
  );

  const menuItems = useCallback(
    (file: FileInfo): MenuEntry[] => {
      const editable = canEdit(file.role);
      const items: MenuEntry[] = [
        { label: 'Open', icon: <ExternalLink size={14} />, onSelect: () => open(file) },
        {
          label: 'Open in new tab',
          icon: <SquareArrowOutUpRight size={14} />,
          onSelect: () => window.open(fileUrl(file), '_blank', 'noopener'),
        },
        { label: 'Copy link', icon: <Link2 size={14} />, onSelect: () => void copyLink(file) },
        { label: 'Share…', icon: <Share2 size={14} />, onSelect: () => setSharing(file) },
      ];
      if (editable) {
        items.push(
          { type: 'separator' },
          { label: 'Rename', icon: <PencilLine size={14} />, onSelect: () => void rename(file) },
          { label: 'Duplicate', icon: <CopyPlus size={14} />, onSelect: () => void duplicate(file) },
          { label: 'Move to project…', icon: <FolderInput size={14} />, onSelect: () => void move(file) },
          { type: 'separator' },
          { label: 'Delete', icon: <Trash size={14} />, danger: true, onSelect: () => void remove(file) },
        );
      }
      return items;
    },
    [copyLink, duplicate, move, open, remove, rename],
  );

  const element = sharing && <ShareDialog open onClose={() => setSharing(null)} file={sharing} />;

  return { open, rename, duplicate, move, moveTo, copyLink, remove, menuItems, element };
}
