import { useState, type FormEvent } from 'react';
import { Link, Navigate, useNavigate, useSearchParams } from 'react-router-dom';
import { api, errorMessage } from '../../lib/api';
import { safeNextPath, useAuth } from '../../lib/auth';
import { useDocumentTitle } from '../../lib/useDocumentTitle';
import { AuthLayout, Field, FormError } from '../../components/AuthLayout';
import { FullScreenLoader, Spinner } from '../../components/StatusScreens';

export default function LoginPage() {
  const { user, setup, loading, setUser } = useAuth();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const invite = params.get('invite');
  const next = invite ? `/invite/${encodeURIComponent(invite)}` : safeNextPath(params.get('next'));
  useDocumentTitle('Log in');

  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (loading) return <FullScreenLoader />;
  if (user && !busy) return <Navigate to={next} replace />;
  if (setup?.needsSetup) return <Navigate to="/setup" replace />;

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      const u = await api.auth.login({ email: email.trim(), password });
      setUser(u);
      navigate(next, { replace: true });
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };

  const signupQuery = new URLSearchParams();
  if (invite) signupQuery.set('invite', invite);
  else if (params.get('next')) signupQuery.set('next', params.get('next') ?? '');
  const signupSearch = signupQuery.toString();
  const signupHref = signupSearch ? `/signup?${signupSearch}` : '/signup';
  const canSignup = Boolean(invite) || setup?.allowSignup !== false;

  return (
    <AuthLayout
      title={invite ? 'Log in to accept your invite' : 'Log in to Flexma'}
      subtitle={invite ? 'Use your existing account, then join the workspace.' : undefined}
      footer={
        canSignup ? (
          <>
            New here? <Link to={signupHref}>Create an account</Link>
          </>
        ) : (
          'Need an account? Ask your Flexma admin for an invite link.'
        )
      }
    >
      <form className="stack" onSubmit={onSubmit}>
        <Field label="Email">
          <input
            className="input input-lg"
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            autoComplete="email"
            required
            autoFocus
          />
        </Field>
        <Field label="Password">
          <input
            className="input input-lg"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
            required
          />
        </Field>
        <FormError message={error} />
        <button type="submit" className="btn btn-primary btn-lg btn-block" disabled={busy}>
          {busy && <Spinner size={14} label="Logging in" />}
          Log in
        </button>
      </form>
    </AuthLayout>
  );
}
