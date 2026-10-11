import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, errorMessage } from '../lib/api';
import { LAST_WORKSPACE_KEY, readStorage } from '../lib/storage';
import { FullScreenLoader, FullScreenMessage } from '../components/StatusScreens';

const DEFAULT_WORKSPACE = 'My Workspace';

let inflight: Promise<string> | null = null;

/** Last used workspace, else the first one; creates "My Workspace" when there is none. */
function resolveHomeWorkspace(): Promise<string> {
  inflight ??= (async () => {
    const list = await api.workspaces.list();
    if (list.length === 0) return (await api.workspaces.create(DEFAULT_WORKSPACE)).id;
    const last = readStorage(LAST_WORKSPACE_KEY);
    return list.find((w) => w.id === last)?.id ?? list[0].id;
  })().finally(() => {
    inflight = null;
  });
  return inflight;
}

/** `/` — sends the user to a workspace dashboard. */
export default function HomeRedirect() {
  const navigate = useNavigate();
  const [error, setError] = useState<unknown>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    resolveHomeWorkspace().then(
      (id) => {
        if (!cancelled) navigate(`/w/${id}`, { replace: true });
      },
      (err: unknown) => {
        if (!cancelled) setError(err);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [navigate, attempt]);

  if (error) {
    return (
      <FullScreenMessage
        title="Couldn't open your workspace"
        message={errorMessage(error)}
        action={{
          label: 'Try again',
          onClick: () => {
            setError(null);
            setAttempt((n) => n + 1);
          },
        }}
      />
    );
  }
  return <FullScreenLoader />;
}
