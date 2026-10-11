import { useState, type FormEvent } from 'react';
import { Link, Navigate, useNavigate, useSearchParams } from 'react-router-dom';
import { api, errorMessage, type InviteInfo } from '../../lib/api';
import { safeNextPath, useAuth } from '../../lib/auth';
import { ROLE_LABELS } from '../../lib/format';
import { useAsync } from '../../lib/useAsync';
import { useDocumentTitle } from '../../lib/useDocumentTitle';
import { AuthLayout, Field, FormError } from '../../components/AuthLayout';
import { FullScreenLoader, Spinner } from '../../components/StatusScreens';

export default function SignupPage() {
  const { user, setup, loading, setUser } = useAuth();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const invite = params.get('invite');
  const next = safeNextPath(params.get('next'));
  useDocumentTitle('Create account');

  const inviteInfo = useAsync<InviteInfo | null>(
    () => (invite ? api.invites.getByToken(invite) : Promise.resolve(null)),
    `invite:${invite ?? ''}`,
  );

  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (loading) return <FullScreenLoader />;
  if (user && !busy) return <Navigate to={invite ? `/invite/${encodeURIComponent(invite)}` : next} replace />;
  if (setup?.needsSetup) return <Navigate to="/setup" replace />;

  const loginHref = invite
    ? `/login?invite=${encodeURIComponent(invite)}`
    : params.get('next')
      ? `/login?next=${encodeURIComponent(next)}`
      : '/login';

  if (!invite && setup && !setup.allowSignup) {
    return (
      <AuthLayout
        title="Sign-ups are closed"
        subtitle="This Flexma instance is invite-only. Ask a workspace owner for an invite link."
        footer={
          <>
            Already have an account? <Link to={loginHref}>Log in</Link>
          </>
        }
      >
        <Link to={loginHref} className="btn btn-lg btn-block">
          Back to log in
        </Link>
      </AuthLayout>
    );
  }

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    if (password.length < 8) {
      setError('Use a password with at least 8 characters.');
      return;
    }
    setBusy(true);
    try {
      const u = await api.auth.signup({
        name: name.trim(),
        email: email.trim(),
        password,
        inviteToken: invite ?? undefined,
      });
      let target = next;
      if (invite) {
        // The server may already have applied the invite during signup; accepting
        // again is harmless and gives us the workspace id to land in.
        try {
          const { workspaceId } = await api.invites.accept(invite);
          target = `/w/${workspaceId}`;
        } catch {
          target = '/';
        }
      }
      setUser(u);
      navigate(target, { replace: true });
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };

  const info = inviteInfo.data;
  const invalidInvite = Boolean(invite) && !inviteInfo.loading && !info;

  return (
    <AuthLayout
      title={info ? `Join ${info.workspaceName}` : 'Create your account'}
      subtitle={
        info
          ? `${info.invitedBy} invited you to collaborate (${ROLE_LABELS[info.role].toLowerCase()}).`
          : 'Design together in real time, on your own server.'
      }
      footer={
        <>
          Already have an account? <Link to={loginHref}>Log in</Link>
        </>
      }
    >
      {invalidInvite && (
        <p className="callout callout-warning">
          This invite link is invalid or has expired. You can still try to sign up, but you may need a new link.
        </p>
      )}
      <form className="stack" onSubmit={onSubmit}>
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
        <FormError message={error} />
        <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={busy}>
          {busy && <Spinner size={14} label="Creating account" />}
          {info ? 'Create account and join' : 'Create account'}
        </button>
      </form>
    </AuthLayout>
  );
}
