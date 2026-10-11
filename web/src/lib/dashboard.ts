import { createContext, useContext, useEffect } from 'react';
import type { Project, Workspace } from './api';

/** Shared state for every page rendered inside the dashboard chrome. */
export interface DashboardContextValue {
  /** All workspaces the user belongs to (undefined while loading). */
  workspaces: Workspace[] | undefined;
  reloadWorkspaces: () => Promise<void>;
  /** The workspace shown in the sidebar: from the URL, else the last used one. */
  workspace: Workspace | null;
  /** Projects of `workspace` (undefined while loading). */
  projects: Project[] | undefined;
  reloadProjects: () => Promise<void>;
  /** Call after creating/moving/deleting files so every open list refreshes. */
  notifyFilesChanged: () => void;
  /** Subscribe to `notifyFilesChanged`. Returns an unsubscribe function. */
  onFilesChanged: (listener: () => void) => () => void;
}

export const DashboardContext = createContext<DashboardContextValue | null>(null);

export function useDashboard(): DashboardContextValue {
  const ctx = useContext(DashboardContext);
  if (!ctx) throw new Error('useDashboard must be used inside the dashboard layout');
  return ctx;
}

/** The current workspace on `/w/:workspaceId/*` routes (the layout guarantees it exists). */
export function useWorkspace(): Workspace {
  const { workspace } = useDashboard();
  if (!workspace) throw new Error('useWorkspace must be used on a /w/:workspaceId route');
  return workspace;
}

/** Run `callback` whenever files change anywhere in the dashboard. Pass a stable callback. */
export function useFilesChanged(callback: () => void): void {
  const { onFilesChanged } = useDashboard();
  useEffect(() => onFilesChanged(callback), [onFilesChanged, callback]);
}
