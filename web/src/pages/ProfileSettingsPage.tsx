import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { LogOut, Monitor, Moon, Sun } from 'lucide-react';
import { api, errorMessage } from '../lib/api';
import { useAuth, useUser } from '../lib/auth';
import { useTheme, type ThemePreference } from '../lib/theme';
import { useDocumentTitle } from '../lib/useDocumentTitle';
import { Avatar } from '../components/Avatar';
import { SettingsSection } from '../components/SettingsSection';
import { Spinner } from '../components/StatusScreens';
import { useToast } from '../components/Toast';
import { PageHeader } from '../components/dashboard/PageHeader';

export default function ProfileSettingsPage() {
  const { setup } = useAuth();
  useDocumentTitle('Profile settings');
  return (
    <div className="page page-narrow">
      <PageHeader title="Profile settings" subtitle="Your account on this Flexma instance." />
      <div className="stack stack-lg">
        <ProfileSection />
        <PasswordSection />
        <AppearanceSection />
        <SessionSection />
        {setup && (
          <p className="text-subtle text-sm">
            {setup.instanceName || 'Flexma'} · version {setup.version}
          </p>
        )}
      </div>
    </div>
  );
}

function ProfileSection() {
  const user = useUser();
  const { setUser } = useAuth();
  const toast = useToast();
  const [name, setName] = useState(user.name);
  const [saving, setSaving] = useState(false);
  const trimmed = name.trim();
  const dirty = trimmed.length > 0 && trimmed !== user.name;

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!dirty) return;
    setSaving(true);
    try {
      setUser(await api.auth.updateMe({ name: trimmed }));
      toast.success('Profile updated');
    } catch (err) {
      toast.error(err);
    } finally {
      setSaving(false);
    }
  };

  return (
    <SettingsSection title="Profile" description="Your name and color identify you to teammates, including on multiplayer cursors.">
      <form className="settings-form" onSubmit={onSubmit}>
        <div className="row row-top">
          <Avatar name={trimmed || user.email} color={user.color} size={48} />
          <div className="stack grow">
            <label className="field">
              <span className="field-label">Name</span>
              <div className="input-row">
                <input
                  className="input"
                  value={name}
                  maxLength={80}
                  autoComplete="name"
                  onChange={(e) => setName(e.target.value)}
                  required
                />
                <button type="submit" className="btn btn-primary" disabled={!dirty || saving}>
                  {saving && <Spinner size={14} label="Saving" />}
                  Save
                </button>
              </div>
            </label>
            <label className="field">
              <span className="field-label">Email</span>
              <input className="input" value={user.email} readOnly />
            </label>
            {user.isAdmin && (
              <p className="field-hint">
                <span className="badge badge-accent">Admin</span> You administer this Flexma instance.
              </p>
            )}
          </div>
        </div>
      </form>
    </SettingsSection>
  );
}

function PasswordSection() {
  const user = useUser();
  const { setUser } = useAuth();
  const toast = useToast();
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [confirm, setConfirm] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    if (next.length < 8) {
      setError('The new password must be at least 8 characters.');
      return;
    }
    if (next !== confirm) {
      setError("The new passwords don't match.");
      return;
    }
    setSaving(true);
    try {
      setUser(await api.auth.updateMe({ password: next, currentPassword: current }));
      setCurrent('');
      setNext('');
      setConfirm('');
      toast.success('Password changed');
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <SettingsSection title="Password" description="Use at least 8 characters.">
      <form className="settings-form stack" onSubmit={onSubmit}>
        {/* Helps password managers associate the change with the account. */}
        <input type="text" name="username" autoComplete="username" value={user.email} readOnly hidden />
        <label className="field">
          <span className="field-label">Current password</span>
          <input
            className="input"
            type="password"
            autoComplete="current-password"
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
            required
          />
        </label>
        <div className="grid-2">
          <label className="field">
            <span className="field-label">New password</span>
            <input
              className="input"
              type="password"
              autoComplete="new-password"
              minLength={8}
              value={next}
              onChange={(e) => setNext(e.target.value)}
              required
            />
          </label>
          <label className="field">
            <span className="field-label">Confirm new password</span>
            <input
              className="input"
              type="password"
              autoComplete="new-password"
              minLength={8}
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              required
            />
          </label>
        </div>
        {error && (
          <p className="form-error" role="alert">
            {error}
          </p>
        )}
        <div>
          <button type="submit" className="btn btn-primary" disabled={saving || !current || !next || !confirm}>
            {saving && <Spinner size={14} label="Saving" />}
            Change password
          </button>
        </div>
      </form>
    </SettingsSection>
  );
}

const THEME_OPTIONS: { value: ThemePreference; label: string; icon: typeof Sun }[] = [
  { value: 'system', label: 'System', icon: Monitor },
  { value: 'light', label: 'Light', icon: Sun },
  { value: 'dark', label: 'Dark', icon: Moon },
];

function AppearanceSection() {
  const { preference, setPreference } = useTheme();
  return (
    <SettingsSection title="Appearance" description="Choose how Flexma looks on this device.">
      <div className="segmented" role="radiogroup" aria-label="Theme">
        {THEME_OPTIONS.map(({ value, label, icon: Icon }) => (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={preference === value}
            className={`segmented-item${preference === value ? ' is-active' : ''}`}
            onClick={() => setPreference(value)}
          >
            <Icon size={14} aria-hidden="true" />
            {label}
          </button>
        ))}
      </div>
    </SettingsSection>
  );
}

function SessionSection() {
  const { logout } = useAuth();
  const navigate = useNavigate();
  return (
    <SettingsSection title="Session" description="Log out of Flexma in this browser.">
      <button
        type="button"
        className="btn"
        onClick={() => void logout().then(() => navigate('/login', { replace: true }))}
      >
        <LogOut size={16} aria-hidden="true" />
        Log out
      </button>
    </SettingsSection>
  );
}
