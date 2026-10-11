import { Bot, ChevronsUpDown, LogOut, Monitor, Moon, Sun, UserRound } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useAuth, useUser } from '../../lib/auth';
import { useTheme, type ThemePreference } from '../../lib/theme';
import { Avatar } from '../Avatar';
import { useMenu, type MenuEntry } from '../Menu';

const THEMES: { value: ThemePreference; label: string; icon: typeof Sun }[] = [
  { value: 'system', label: 'System theme', icon: Monitor },
  { value: 'light', label: 'Light', icon: Sun },
  { value: 'dark', label: 'Dark', icon: Moon },
];

export function UserMenu() {
  const user = useUser();
  const { logout } = useAuth();
  const { preference, setPreference } = useTheme();
  const navigate = useNavigate();
  const menu = useMenu();

  const items: MenuEntry[] = [
    { type: 'header', label: user.email },
    { label: 'Profile settings', icon: <UserRound size={14} />, onSelect: () => navigate('/settings') },
    { label: 'AI agents & tokens', icon: <Bot size={14} />, onSelect: () => navigate('/agents') },
    { type: 'separator' },
    ...THEMES.map<MenuEntry>((t) => ({
      label: t.label,
      icon: <t.icon size={14} />,
      checked: preference === t.value,
      onSelect: () => setPreference(t.value),
    })),
    { type: 'separator' },
    {
      label: 'Log out',
      icon: <LogOut size={14} />,
      onSelect: () => {
        void logout().then(() => navigate('/login', { replace: true }));
      },
    },
  ];

  return (
    <>
      <button
        type="button"
        className="sidebar-user"
        aria-haspopup="menu"
        aria-expanded={menu.isOpen}
        onClick={(e) => menu.open(e.currentTarget, items, { label: 'Account', minWidth: 'anchor' })}
      >
        <Avatar name={user.name || user.email} color={user.color} size={24} />
        <span className="sidebar-user-text">
          <span className="sidebar-user-name">{user.name || user.email}</span>
          {user.isAdmin && <span className="badge badge-accent">Admin</span>}
        </span>
        <ChevronsUpDown size={14} className="sidebar-user-chevron" aria-hidden="true" />
      </button>
      {menu.element}
    </>
  );
}
