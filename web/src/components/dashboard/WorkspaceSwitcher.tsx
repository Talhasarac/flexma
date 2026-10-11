import { ChevronDown, Plus, Settings } from 'lucide-react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useDashboard } from '../../lib/dashboard';
import { colorFromString, pluralize, ROLE_LABELS } from '../../lib/format';
import { Avatar } from '../Avatar';
import { useMenu, type MenuEntry } from '../Menu';
import { useDashboardActions } from './useDashboardActions';

export function WorkspaceSwitcher() {
  const { workspaces, workspace } = useDashboard();
  const { createWorkspace } = useDashboardActions();
  const navigate = useNavigate();
  const location = useLocation();
  const menu = useMenu();

  const switchTo = (id: string) => {
    const onSettings = /^\/w\/[^/]+\/settings/.test(location.pathname);
    navigate(onSettings ? `/w/${id}/settings` : `/w/${id}`);
  };

  const items: MenuEntry[] = [
    { type: 'header', label: 'Workspaces' },
    ...(workspaces ?? []).map<MenuEntry>((w) => ({
      label: (
        <span className="ws-menu-row">
          <span className="ws-menu-name">{w.name}</span>
          <span className="ws-menu-meta">
            {ROLE_LABELS[w.role]} · {pluralize(w.memberCount, 'member')}
          </span>
        </span>
      ),
      icon: <Avatar name={w.name} color={colorFromString(w.id)} size={20} square />,
      checked: w.id === workspace?.id,
      onSelect: () => switchTo(w.id),
    })),
    { type: 'separator' },
    { label: 'New workspace', icon: <Plus size={14} />, onSelect: () => void createWorkspace() },
  ];
  if (workspace) {
    items.push({
      label: 'Workspace settings',
      icon: <Settings size={14} />,
      onSelect: () => navigate(`/w/${workspace.id}/settings`),
    });
  }

  const name = workspace?.name ?? (workspaces && workspaces.length === 0 ? 'No workspace' : 'Loading…');

  return (
    <>
      <button
        type="button"
        className="ws-switcher"
        aria-haspopup="menu"
        aria-expanded={menu.isOpen}
        onClick={(e) => menu.open(e.currentTarget, items, { label: 'Switch workspace', minWidth: 'anchor' })}
      >
        <Avatar name={workspace?.name ?? '?'} color={workspace ? colorFromString(workspace.id) : '#9a9aa3'} size={28} square />
        <span className="ws-switcher-text">
          <span className="ws-switcher-name">{name}</span>
          {workspace && (
            <span className="ws-switcher-sub">{pluralize(workspace.memberCount, 'member')}</span>
          )}
        </span>
        <ChevronDown size={14} className="ws-switcher-chevron" aria-hidden="true" />
      </button>
      {menu.element}
    </>
  );
}
