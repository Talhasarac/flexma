// Public view-only viewer for share links (/s/:token). No sign-in needed:
// guests watch the file live, inspect layers and export, but can't edit or
// see comments. Signed-in workspace members are sent to the full editor.
import { useEffect, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { CloudOff, Download, Eye } from 'lucide-react';
import { api, errorMessage, isApiError, type SharedFileInfo } from '../lib/api';
import { useAuth } from '../lib/auth';
import { Logo } from '../components/Logo';
import { MadeBy } from '../components/MadeBy';
import { useMenu } from '../components/Menu';
import { FullScreenLoader, FullScreenMessage } from '../components/StatusScreens';
import { useDocumentTitle } from '../lib/useDocumentTitle';
import { createEditor, loadEngine } from '../engine';
import { Editor } from './EditorPage';
import { EditorStore, useStatus, useUi } from './store';
import { Presence, ZoomButton } from './TopBar';
import './editor.css';

const DEAD_LINK = 'This link was turned off, reset or has expired. Ask the person who shared it for a new one.';

export default function SharePage() {
  const { token = '' } = useParams();
  const navigate = useNavigate();
  const { user, loading } = useAuth();
  const userId = user?.id ?? null;
  const [info, setInfo] = useState<SharedFileInfo | null>(null);
  const [store, setStore] = useState<EditorStore | null>(null);
  const [canvas] = useState(() => document.createElement('canvas'));
  const [error, setError] = useState<string | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  useDocumentTitle(info?.name);

  useEffect(() => {
    if (loading) return;
    let cancelled = false;
    let created: EditorStore | null = null;
    setError(null);
    Promise.all([api.share.info(token), loadEngine()])
      .then(async ([shared]) => {
        if (userId) {
          const member = await api.files.get(shared.fileId).then(
            () => true,
            () => false,
          );
          if (cancelled) return;
          if (member) {
            navigate(`/file/${shared.fileId}`, { replace: true });
            return;
          }
        }
        if (cancelled) return;
        const engine = await createEditor(canvas);
        if (cancelled) {
          engine.free();
          return;
        }
        setInfo(shared);
        created = new EditorStore(engine, shared.fileId, { shareToken: token });
        created.onFatal = (m) => setFatal(m);
        created.connect(api.share.urls.socket(token));
        // Handy for debugging and profiling from the console.
        (window as unknown as { __flexma?: EditorStore }).__flexma = created;
        setStore(created);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(isApiError(err, 404) ? DEAD_LINK : errorMessage(err, 'Could not open this link'));
      });
    return () => {
      cancelled = true;
      created?.close();
      created?.engine.free();
    };
  }, [token, canvas, loading, userId, navigate]);

  if (error) return <FullScreenMessage title="Link unavailable" message={error} homeLink={!!userId} />;
  if (fatal) return <FullScreenMessage title="Link unavailable" message={fatal === 'This share link is no longer active.' ? DEAD_LINK : fatal} homeLink={!!userId} />;
  if (!info || !store) return <FullScreenLoader label="Opening shared file" />;
  return (
    <Editor
      store={store}
      canvas={canvas}
      header={<ShareTopBar store={store} info={info} signedIn={!!userId} />}
      footer={
        <div className="share-credit">
          <MadeBy compact />
        </div>
      }
    />
  );
}

function ShareTopBar({ store, info, signedIn }: { store: EditorStore; info: SharedFileInfo; signedIn: boolean }) {
  const ui = useUi(store);
  const status = useStatus(store);
  const menu = useMenu();
  const next = encodeURIComponent(window.location.pathname);
  return (
    <header className="editor-topbar">
      <div className="tb-left">
        <Link to="/" className="tb-logo" aria-label="Flexma home">
          <Logo size={22} />
        </Link>
        <div className="tb-title">
          <span className="tb-filename tb-filename-static" title={info.name}>
            {info.name}
          </span>
          <span className="tb-badge">
            <Eye size={12} /> View only
          </span>
          <span className="tb-shared-by">
            {info.sharedBy ? `Shared by ${info.sharedBy} · ` : ''}
            {info.workspaceName}
          </span>
          {status === 'offline' && (
            <span className="tb-status" title="Offline — reconnecting">
              <CloudOff size={13} />
            </span>
          )}
        </div>
      </div>
      <div className="tb-right">
        <Presence store={store} self={null} />
        <button
          type="button"
          className="btn btn-sm"
          onClick={(ev) =>
            menu.open(
              ev.currentTarget,
              [
                { label: 'Export page as PNG', onSelect: () => window.open(store.exportPngUrl({ page: ui.page, scale: 2 }), '_blank') },
                { label: 'Export page as SVG', onSelect: () => window.open(store.exportSvgUrl({ page: ui.page }), '_blank') },
              ],
              { align: 'end' },
            )
          }
        >
          <Download size={14} /> Export
        </button>
        {!signedIn && (
          <Link to={`/login?next=${next}`} className="btn btn-sm btn-primary">
            Log in
          </Link>
        )}
        <ZoomButton store={store} />
      </div>
      {menu.element}
    </header>
  );
}
