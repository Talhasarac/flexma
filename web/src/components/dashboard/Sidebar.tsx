import { useState, type DragEvent } from 'react';
import { NavLink } from 'react-router-dom';
import { Bot, Clock, Ellipsis, Folder, FolderOpen, PencilLine, Plus, Settings, Shield, Trash } from 'lucide-react';
import type { Project } from '../../lib/api';
import { useAuth } from '../../lib/auth';
import { useDashboard } from '../../lib/dashboard';
import { canEdit } from '../../lib/format';
import { Logo } from '../Logo';
import { MadeBy } from '../MadeBy';
import { contextMenuAnchor, useMenu, type MenuAnchor } from '../Menu';
import { FILE_DRAG_TYPE, type DraggedFile } from './FileGrid';
import { useDashboardActions } from './useDashboardActions';
import { useFileActions } from './useFileActions';
import { UserMenu } from './UserMenu';
import { WorkspaceSwitcher } from './WorkspaceSwitcher';

const linkClass = ({ isActive }: { isActive: boolean }) => `sidebar-link${isActive ? ' is-active' : ''}`;

export function Sidebar() {
  const { workspace, projects } = useDashboard();
  const { setup, user } = useAuth();
  const { createProject, renameProject, deleteProject } = useDashboardActions();
  const { moveTo } = useFileActions();
  const menu = useMenu();
  const editable = canEdit(workspace?.role);

  const sorted = [...(projects ?? [])].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }));

  const openProjectMenu = (anchor: MenuAnchor, project: Project) =>
    menu.open(
      anchor,
      [
        { label: 'Rename', icon: <PencilLine size={14} />, onSelect: () => void renameProject(project) },
        { type: 'separator' },
        { label: 'Delete project', icon: <Trash size={14} />, danger: true, onSelect: () => void deleteProject(project) },
      ],
      { label: `${project.name} actions` },
    );

  return (
    <div className="sidebar-inner">
      <div className="sidebar-top">
        <WorkspaceSwitcher />
      </div>

      <nav className="sidebar-scroll" aria-label="Main">
        {workspace && (
          <div className="sidebar-group">
            <NavLink to={`/w/${workspace.id}`} end className={linkClass}>
              <Clock size={16} aria-hidden="true" />
              <span>Recent</span>
            </NavLink>
            <NavLink to="/agents" className={linkClass}>
              <Bot size={16} aria-hidden="true" />
              <span>AI Agents</span>
            </NavLink>
            <NavLink to={`/w/${workspace.id}/settings`} className={linkClass}>
              <Settings size={16} aria-hidden="true" />
              <span>Settings</span>
            </NavLink>
            {user?.isAdmin && (
              <NavLink to="/admin" className={linkClass}>
                <Shield size={16} aria-hidden="true" />
                <span>Admin</span>
              </NavLink>
            )}
          </div>
        )}
        {!workspace && (
          <div className="sidebar-group">
            <NavLink to="/agents" className={linkClass}>
              <Bot size={16} aria-hidden="true" />
              <span>AI Agents</span>
            </NavLink>
          </div>
        )}

        {workspace && (
          <div className="sidebar-group">
            <div className="sidebar-section-header">
              <span>Projects</span>
              {editable && (
                <button
                  type="button"
                  className="btn btn-ghost btn-icon btn-xs"
                  aria-label="New project"
                  title="New project"
                  onClick={() => void createProject()}
                >
                  <Plus size={14} />
                </button>
              )}
            </div>
            {projects === undefined ? (
              <div className="sidebar-skeleton" aria-hidden="true">
                <span className="skeleton skeleton-text" style={{ width: '70%' }} />
                <span className="skeleton skeleton-text" style={{ width: '55%' }} />
                <span className="skeleton skeleton-text" style={{ width: '62%' }} />
              </div>
            ) : sorted.length === 0 ? (
              <p className="sidebar-hint">
                No projects yet.
                {editable && (
                  <>
                    {' '}
                    <button type="button" className="link-button" onClick={() => void createProject()}>
                      Create one
                    </button>
                  </>
                )}
              </p>
            ) : (
              <ul className="sidebar-list">
                {sorted.map((p) => (
                  <ProjectLink
                    key={p.id}
                    project={p}
                    editable={editable}
                    onMenu={(anchor) => openProjectMenu(anchor, p)}
                    onDropFile={(file) => void moveTo(file, p.id)}
                  />
                ))}
              </ul>
            )}
          </div>
        )}
      </nav>

      <div className="sidebar-bottom">
        <UserMenu />
        <div className="sidebar-brand">
          <Logo size={14} wordmark />
          {setup?.version && <span className="sidebar-version">v{setup.version}</span>}
          <MadeBy compact />
        </div>
      </div>
      {menu.element}
    </div>
  );
}

interface ProjectLinkProps {
  project: Project;
  editable: boolean;
  onMenu: (anchor: MenuAnchor) => void;
  onDropFile: (file: DraggedFile) => void;
}

function ProjectLink({ project, editable, onMenu, onDropFile }: ProjectLinkProps) {
  const [dropping, setDropping] = useState(false);

  const accepts = (e: DragEvent) => editable && e.dataTransfer.types.includes(FILE_DRAG_TYPE);

  return (
    <li
      className={`sidebar-project${dropping ? ' is-drop-target' : ''}`}
      onDragOver={(e) => {
        if (!accepts(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        setDropping(true);
      }}
      onDragLeave={() => setDropping(false)}
      onDrop={(e) => {
        setDropping(false);
        if (!accepts(e)) return;
        e.preventDefault();
        try {
          const file = JSON.parse(e.dataTransfer.getData(FILE_DRAG_TYPE)) as DraggedFile;
          if (file.projectId !== project.id) onDropFile(file);
        } catch {
          // ignore malformed drops
        }
      }}
      onContextMenu={(e) => {
        if (!editable) return;
        e.preventDefault();
        onMenu(contextMenuAnchor(e));
      }}
    >
      <NavLink to={`/w/${project.workspaceId}/p/${project.id}`} className={linkClass}>
        {({ isActive }) => (
          <>
            {isActive ? <FolderOpen size={16} aria-hidden="true" /> : <Folder size={16} aria-hidden="true" />}
            <span className="sidebar-link-label">{project.name}</span>
            {project.fileCount > 0 && <span className="sidebar-count">{project.fileCount}</span>}
          </>
        )}
      </NavLink>
      {editable && (
        <button
          type="button"
          className="btn btn-ghost btn-icon btn-xs sidebar-project-more"
          aria-label={`Actions for ${project.name}`}
          aria-haspopup="menu"
          onClick={(e) => onMenu(e.currentTarget)}
        >
          <Ellipsis size={14} />
        </button>
      )}
    </li>
  );
}
