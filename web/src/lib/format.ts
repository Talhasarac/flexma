import type { Role } from './api';

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "just now", "5m ago", "2h ago", "3d ago", then "Mar 4" / "Mar 4, 2023". */
export function formatRelativeTime(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return 'never';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '';
  const diff = now - t;
  if (diff < 45_000) return 'just now';
  if (diff < HOUR) return `${Math.max(1, Math.round(diff / MINUTE))}m ago`;
  if (diff < DAY) return `${Math.round(diff / HOUR)}h ago`;
  if (diff < 7 * DAY) {
    const days = Math.round(diff / DAY);
    return days === 1 ? 'yesterday' : `${days}d ago`;
  }
  const date = new Date(t);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  return date.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
  });
}

/** Full, locale formatted date + time (for tooltips). */
export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return '';
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '';
  return new Date(t).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** Up to two initials from a name or email. */
export function initials(name: string): string {
  const clean = name.trim().replace(/@.*$/, '');
  if (!clean) return '?';
  const parts = clean.split(/[\s._-]+/).filter(Boolean);
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

const PALETTE = ['#6d5dfc', '#e5484d', '#f76b15', '#ffb224', '#30a46c', '#12a594', '#0090ff', '#d6409f', '#8e4ec6', '#3e63dd'];

/** Stable color for things without one (e.g. workspaces). */
export function colorFromString(s: string): string {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return PALETTE[Math.abs(h) % PALETTE.length];
}

/** Black or white, whichever reads better on the given hex background. */
export function readableTextColor(hex: string): string {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return '#ffffff';
  let h = m[1];
  if (h.length === 3) h = h.split('').map((c) => c + c).join('');
  const r = parseInt(h.slice(0, 2), 16) / 255;
  const g = parseInt(h.slice(2, 4), 16) / 255;
  const b = parseInt(h.slice(4, 6), 16) / 255;
  const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  const lum = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  return lum > 0.45 ? '#1d1d1f' : '#ffffff';
}

export const ROLE_LABELS: Record<Role, string> = {
  owner: 'Owner',
  editor: 'Can edit',
  viewer: 'Can view',
};

export function canEdit(role: Role | undefined): boolean {
  return role === 'owner' || role === 'editor';
}

export function pluralize(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
