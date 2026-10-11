import { useState, type FormEvent } from 'react';
import { Navigate, useNavigate, useSearchParams } from 'react-router-dom';
import { api, errorMessage } from '../../lib/api';
import { useAuth } from '../../lib/auth';
import { useDocumentTitle } from '../../lib/useDocumentTitle';
import { AuthLayout, Field, FormError } from '../../components/AuthLayout';
import { FullScreenLoader, Spinner } from '../../components/StatusScreens';
import { useToast } from '../../components/Toast';

/** First-run screen: create the admin account and the first workspace. */
export default function SetupPage() {
  const { user, setup, loading, setUser, refresh } = useAuth();
  const navigate = useNavigate();
  const toast = useToast();
  const [params] = useSearchParams();
  useDocumentTitle('Set up Flexma');

  // The setup link printed in the server log carries ?token=…
  const linkToken = params.get('token') ?? '';
  const [setupToken, setSetupToken] = useState(linkToken);
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [workspaceName, setWorkspaceName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (loading) return <FullScreenLoader />;
  // Setup already done (or someone else just finished it).
  if (!busy && (!setup?.needsSetup || user)) return <Navigate to={user ? '/' : '/login'} replace />;

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    if (password.length < 8) {
      setError('Use a password with at least 8 characters.');
      return;
    }
    setBusy(true);
    let created;
    try {
      created = await api.auth.signup({ name: name.trim(), email: email.trim(), password, setupToken: setupToken.trim() });
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
      return;
    }
    let target = '/';
    try {
      const ws = await api.workspaces.create(workspaceName.trim() || 'My Workspace');
      target = `/w/${ws.id}`;
    } catch (err) {
      toast.error(`Your account is ready, but the workspace couldn't be created: ${errorMessage(err)}`);
    }
    setUser(created);
    void refresh();
    navigate(target, { replace: true });
  };

  return (
    <AuthLayout
      wide
      title="Welcome to Flexma"
      subtitle="Let's set up this instance. You'll be its administrator, and can invite your team right after."
    >
      <form className="stack" onSubmit={onSubmit}>
        {!linkToken && (
          <Field
            label="Setup token"
            hint="Only the person running the server can set it up. Open the setup link from the server log (journalctl -u flexma, or docker compose logs flexma), or paste its token here."
          >
            <input
              className="input input-lg"
              value={setupToken}
              onChange={(e) => setSetupToken(e.target.value)}
              autoComplete="off"
              spellCheck={false}
              required
            />
          </Field>
        )}
        <div className="form-section-label">Your admin account</div>
        <Field label="Name">
          <input
            className="input input-lg"
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoComplete="name"
            required
            autoFocus
          />
        </Field>
        <Field label="Email">
          <input
            className="input input-lg"
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            autoComplete="email"
            required
          />
        </Field>
        <Field label="Password" hint="At least 8 characters.">
          <input
            className="input input-lg"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="new-password"
            minLength={8}
            required
          />
        </Field>
        <div className="form-section-label">Your first workspace</div>
        <Field label="Workspace name" hint="Usually your team or company name. You can change it later.">
          <input
            className="input input-lg"
            value={workspaceName}
            placeholder="My Workspace"
            onChange={(e) => setWorkspaceName(e.target.value)}
            maxLength={80}
          />
        </Field>
        <FormError message={error} />
        <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={busy}>
          {busy && <Spinner size={14} label="Setting up" />}
          Create account and workspace
        </button>
      </form>
    </AuthLayout>
  );
}
