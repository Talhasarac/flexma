import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, Outlet, useLocation, useParams } from 'react-router-dom';
import { Menu as MenuIcon, SearchX } from 'lucide-react';
import { api, type Project } from '../../lib/api';
import { DashboardContext, type DashboardContextValue } from '../../lib/dashboard';
import { preloadEditorWhenIdle } from '../../lib/preload';
import { LAST_WORKSPACE_KEY, readStorage, writeStorage } from '../../lib/storage';
import { useAsync } from '../../lib/useAsync';
import { EmptyState, ErrorState } from '../EmptyState';
import { Logo } from '../Logo';
import { FileGridSkeleton } from './FileGrid';
import { Sidebar } from './Sidebar';

/**
 * Dashboard chrome: sidebar (workspace switcher, nav, projects, account) and
 * the routed page. Provides `useDashboard()` to the pages.
 */
export default function DashboardLayout() {
  const { workspaceId } = useParams();
  const location = useLocation();
  const [navOpen, setNavOpen] = useState(false);

  useEffect(() => preloadEditorWhenIdle(), []);

  const workspacesQuery = useAsync(() => api.workspaces.list(), 'workspaces');
  const workspaces = workspacesQuery.data;

  const currentId = useMemo(() => {
    if (workspaceId) return workspaceId;
    if (!workspaces || workspaces.length === 0) return null;
    const last = readStorage(LAST_WORKSPACE_KEY);
    return workspaces.find((w) => w.id === last)?.id ?? workspaces[0].id;
  }, [workspaceId, workspaces]);

  const workspace = workspaces?.find((w) => w.id === currentId) ?? null;

  useEffect(() => {
    if (workspaceId && workspace) writeStorage(LAST_WORKSPACE_KEY, workspaceId);
  }, [workspaceId, workspace]);

  const projectsQuery = useAsync<Project[]>(
    () => (currentId ? api.projects.list(currentId) : Promise.resolve([])),
    `projects:${currentId ?? ''}`,
  );
  const reloadProjects = projectsQuery.reload;

  const listeners = useRef(new Set<() => void>());
  const onFilesChanged = useCallback((listener: () => void) => {
    listeners.current.add(listener);
    return () => {
      listeners.current.delete(listener);
    };
  }, []);
  const notifyFilesChanged = useCallback(() => {
    listeners.current.forEach((l) => l());
    void reloadProjects();
  }, [reloadProjects]);

  const value = useMemo<DashboardContextValue>(
    () => ({
      workspaces,
      reloadWorkspaces: workspacesQuery.reload,
      workspace,
      projects: projectsQuery.data,
      reloadProjects,
      notifyFilesChanged,
      onFilesChanged,
    }),
    [workspaces, workspacesQuery.reload, workspace, projectsQuery.data, reloadProjects, notifyFilesChanged, onFilesChanged],
  );

  // Close the mobile drawer on navigation and with Escape.
  const [lastPath, setLastPath] = useState(location.pathname);
  if (lastPath !== location.pathname) {
    setLastPath(location.pathname);
    setNavOpen(false);
  }
  // The main column is the scroll container: start each page at the top.
  const mainRef = useRef<HTMLElement>(null);
  useEffect(() => {
    mainRef.current?.scrollTo(0, 0);
  }, [location.pathname]);
  useEffect(() => {
    if (!navOpen) return;
    document.getElementById('dashboard-sidebar')?.querySelector<HTMLElement>('button, a')?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setNavOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [navOpen]);

  let content: ReactNode = <Outlet />;
  if (workspaceId) {
    if (!workspaces) {
      content = workspacesQuery.error ? (
        <ErrorState error={workspacesQuery.error} onRetry={() => void workspacesQuery.reload()} />
      ) : (
        <PageSkeleton />
      );
    } else if (!workspace) {
      content = (
        <EmptyState
          icon={<SearchX size={24} />}
          title="Workspace not found"
          description="It may have been deleted, or you don't have access to it anymore."
          action={
            <Link to="/" className="btn">
              Go to your workspaces
            </Link>
          }
        />
      );
    }
  }

  return (
    <DashboardContext value={value}>
      <div className={`dashboard${navOpen ? ' nav-open' : ''}`}>
        <header className="mobile-topbar">
          <button
            type="button"
            className="btn btn-ghost btn-icon"
            aria-label="Open navigation"
            aria-expanded={navOpen}
            aria-controls="dashboard-sidebar"
            onClick={() => setNavOpen(true)}
          >
            <MenuIcon size={18} />
          </button>
          <Link to="/" className="mobile-topbar-brand">
            <Logo size={20} wordmark />
          </Link>
        </header>
        <aside id="dashboard-sidebar" className="sidebar" aria-label="Sidebar">
          <Sidebar />
        </aside>
        <div className="sidebar-scrim" aria-hidden="true" onClick={() => setNavOpen(false)} />
        <main ref={mainRef} className="dashboard-main">
          {content}
        </main>
      </div>
    </DashboardContext>
  );
}

function PageSkeleton() {
  return (
    <div className="page" aria-busy="true">
      <div className="page-header">
        <span className="skeleton skeleton-title" />
      </div>
      <FileGridSkeleton />
    </div>
  );
}
