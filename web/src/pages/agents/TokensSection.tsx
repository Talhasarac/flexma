import { useState, type FormEvent } from 'react';
import { KeyRound, Plus, TriangleAlert } from 'lucide-react';
import { api, type CreatedApiToken } from '../../lib/api';
import { useAsync } from '../../lib/useAsync';
import { CopyField } from '../../components/CopyField';
import { useDialogs } from '../../components/Dialogs';
import { ErrorState } from '../../components/EmptyState';
import { Spinner } from '../../components/StatusScreens';
import { TimeAgo } from '../../components/TimeAgo';
import { useToast } from '../../components/Toast';

interface TokensSectionProps {
  /** The token created in this session (shown once). */
  fresh: CreatedApiToken | null;
  onCreated: (token: CreatedApiToken) => void;
  onDismissFresh: () => void;
}

export function TokensSection({ fresh, onCreated, onDismissFresh }: TokensSectionProps) {
  const toast = useToast();
  const dialogs = useDialogs();
  const tokens = useAsync(() => api.tokens.list(), 'tokens');
  const [name, setName] = useState('');
  const [creating, setCreating] = useState(false);

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setCreating(true);
    try {
      const created = await api.tokens.create(name.trim() || defaultTokenName());
      onCreated(created);
      setName('');
      void tokens.reload();
    } catch (err) {
      toast.error(err);
    } finally {
      setCreating(false);
    }
  };

  const revoke = async (id: string, tokenName: string) => {
    const ok = await dialogs.confirm({
      title: `Revoke “${tokenName}”?`,
      message: 'Agents using this token are disconnected immediately and will need a new token.',
      confirmLabel: 'Revoke token',
      danger: true,
    });
    if (!ok) return;
    try {
      await api.tokens.delete(id);
      tokens.setData((list) => (list ?? []).filter((t) => t.id !== id));
      if (fresh?.id === id) onDismissFresh();
      toast.success('Token revoked');
    } catch (err) {
      toast.error(err);
    }
  };

  return (
    <div className="stack">
      <form className="input-row" onSubmit={onSubmit}>
        <input
          className="input"
          value={name}
          maxLength={80}
          placeholder="Token name, e.g. Claude Code on my laptop"
          aria-label="Token name"
          onChange={(e) => setName(e.target.value)}
        />
        <button type="submit" className="btn btn-primary" disabled={creating}>
          {creating ? <Spinner size={14} label="Creating" /> : <Plus size={16} aria-hidden="true" />}
          Create token
        </button>
      </form>

      {fresh && (
        <div className="callout callout-warning" role="status">
          <div className="callout-title">
            <TriangleAlert size={16} aria-hidden="true" />
            Copy “{fresh.name}” now. You won't be able to see it again.
          </div>
          <CopyField value={fresh.token} toastMessage="Token copied" emphasis />
          <p className="text-muted text-sm">
            The setup commands below now include this token, so you can paste them straight into your terminal.
          </p>
          <div>
            <button type="button" className="btn btn-sm" onClick={onDismissFresh}>
              I've saved it
            </button>
          </div>
        </div>
      )}

      {!tokens.data && tokens.error ? (
        <ErrorState error={tokens.error} onRetry={() => void tokens.reload()} title="Couldn't load tokens" />
      ) : !tokens.data ? (
        <div className="list-placeholder">
          <span className="skeleton skeleton-text" style={{ width: '50%' }} />
        </div>
      ) : tokens.data.length === 0 ? (
        <p className="text-muted">No tokens yet. Create one for each agent or machine you connect.</p>
      ) : (
        <ul className="list">
          {tokens.data.map((t) => (
            <li key={t.id} className="list-row">
              <span className="list-row-icon" aria-hidden="true">
                <KeyRound size={16} />
              </span>
              <span className="list-row-main">
                <span className="list-row-title">
                  {t.name}
                  <code className="token-prefix">{t.prefix}…</code>
                </span>
                <span className="list-row-sub">
                  <TimeAgo date={t.createdAt} prefix="Created" />
                  {' · '}
                  {t.lastUsedAt ? <TimeAgo date={t.lastUsedAt} prefix="last used" /> : 'never used'}
                </span>
              </span>
              <span className="list-row-actions">
                <button type="button" className="btn btn-sm" onClick={() => void revoke(t.id, t.name)}>
                  Revoke
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function defaultTokenName(): string {
  const date = new Date().toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  return `Agent token (${date})`;
}
