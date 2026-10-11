import { useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { api, errorMessage, isApiError } from '../../lib/api';
import { useAuth } from '../../lib/auth';
import { colorFromString, ROLE_LABELS } from '../../lib/format';
import { useAsync } from '../../lib/useAsync';
import { useDocumentTitle } from '../../lib/useDocumentTitle';
import { AuthLayout, FormError } from '../../components/AuthLayout';
import { Avatar } from '../../components/Avatar';
import { FullScreenLoader, Spinner } from '../../components/StatusScreens';
import { useToast } from '../../components/Toast';

/** `/invite/:token` — shows who invited you where, and lets you join. */
export default function InvitePage() {
  const { token = '' } = useParams();
  const { user, loading, logout } = useAuth();
  const navigate = useNavigate();
  const toast = useToast();
  const invite = useAsync(() => api.invites.getByToken(token), token);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useDocumentTitle(invite.data ? `Join ${invite.data.workspaceName}` : 'Invite');

  if (loading || (invite.loading && !invite.data)) return <FullScreenLoader />;

  if (!invite.data) {
    const notFound = isApiError(invite.error, 404) || isApiError(invite.error, 410);
    return (
      <AuthLayout
        title={notFound ? 'This invite is no longer valid' : "Couldn't open this invite"}
        subtitle={
          notFound
            ? 'The link may have expired or been revoked. Ask the person who invited you for a new one.'
            : errorMessage(invite.error)
        }
      >
        <div className="stack">
          {!notFound && (
            <button type="button" className="btn btn-lg btn-block" onClick={() => void invite.reload()}>
              Try again
            </button>
          )}
          <Link to="/" className="btn btn-lg btn-block">
            Go to Flexma
          </Link>
        </div>
      </AuthLayout>
    );
  }

  const info = invite.data;
  const enc = encodeURIComponent(token);

  const accept = async () => {
    setBusy(true);
    setError(null);
    try {
      const { workspaceId } = await api.invites.accept(token);
      toast.success(`Welcome to ${info.workspaceName}!`);
      navigate(`/w/${workspaceId}`, { replace: true });
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      title={`Join ${info.workspaceName}`}
      subtitle={
        <>
          <strong className="text-strong">{info.invitedBy}</strong> invited you to collaborate on Flexma.
        </>
      }
      footer={
        user ? (
          <>
            Signed in as {user.email}.{' '}
            <button
              type="button"
              className="link-button"
              onClick={() => void logout().then(() => navigate(`/login?invite=${enc}`, { replace: true }))}
            >
              Use another account
            </button>
          </>
        ) : undefined
      }
    >
      <div className="invite-summary">
        <Avatar name={info.workspaceName} color={colorFromString(info.workspaceName)} size={40} square />
        <div>
          <div className="invite-summary-name">{info.workspaceName}</div>
          <div className="text-muted">You'll join as: {ROLE_LABELS[info.role]}</div>
        </div>
      </div>
      <FormError message={error} />
      {user ? (
        <button type="button" className="btn btn-primary btn-lg btn-block" disabled={busy} onClick={() => void accept()}>
          {busy && <Spinner size={14} label="Joining" />}
          Accept invite
        </button>
      ) : (
        <div className="stack">
          <Link to={`/signup?invite=${enc}`} className="btn btn-primary btn-lg btn-block">
            Create an account to join
          </Link>
          <Link to={`/login?invite=${enc}`} className="btn btn-lg btn-block">
            I already have an account
          </Link>
        </div>
      )}
    </AuthLayout>
  );
}
