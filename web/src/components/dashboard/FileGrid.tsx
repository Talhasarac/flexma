import { useState, type DragEvent, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { Brush, Ellipsis, FileImage, SearchX } from 'lucide-react';
import { api, type FileInfo } from '../../lib/api';
import { canEdit } from '../../lib/format';
import { EmptyState, ErrorState } from '../EmptyState';
import { contextMenuAnchor, useMenu, type MenuAnchor } from '../Menu';
import { TimeAgo } from '../TimeAgo';
import { fileUrl, useFileActions } from './useFileActions';

/** dataTransfer type used when dragging a file card onto a project. */
export const FILE_DRAG_TYPE = 'application/x-flexma-file';

export interface DraggedFile {
  id: string;
  projectId: string;
  name: string;
}

interface FileGridProps {
  files: FileInfo[] | undefined;
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  /** Show the project name under each file (Recent view). */
  showProject?: boolean;
  /** Shown when there are no files at all. */
  empty: ReactNode;
  /** Case-insensitive name filter. */
  filter?: string;
}

export function FileGrid({ files, loading, error, onRetry, showProject, empty, filter = '' }: FileGridProps) {
  const menu = useMenu();
  const actions = useFileActions();

  if (!files) {
    if (error && !loading) return <ErrorState error={error} onRetry={onRetry} title="Couldn't load files" />;
    return <FileGridSkeleton />;
  }
  if (files.length === 0) return <>{empty}</>;

  const q = filter.trim().toLowerCase();
  const shown = q ? files.filter((f) => f.name.toLowerCase().includes(q)) : files;
  if (shown.length === 0) {
    return (
      <EmptyState
        compact
        icon={<SearchX size={22} />}
        title="No matching files"
        description={`Nothing here matches “${filter.trim()}”.`}
      />
    );
  }

  const openMenu = (anchor: MenuAnchor, file: FileInfo) =>
    menu.open(anchor, actions.menuItems(file), { label: `${file.name} actions`, align: 'end' });

  return (
    <>
      <ul className="file-grid" aria-label="Files">
        {shown.map((file) => (
          <FileCard
            key={file.id}
            file={file}
            showProject={showProject}
            onMenu={(anchor) => openMenu(anchor, file)}
            onRename={() => void actions.rename(file)}
            onDelete={() => void actions.remove(file)}
          />
        ))}
      </ul>
      {menu.element}
      {actions.element}
    </>
  );
}

interface FileCardProps {
  file: FileInfo;
  showProject?: boolean;
  onMenu: (anchor: MenuAnchor) => void;
  onRename: () => void;
  onDelete: () => void;
}

function FileCard({ file, showProject, onMenu, onRename, onDelete }: FileCardProps) {
  const editable = canEdit(file.role);
  const [dragging, setDragging] = useState(false);

  const onDragStart = (e: DragEvent<HTMLLIElement>) => {
    if (!editable) return;
    const payload: DraggedFile = { id: file.id, projectId: file.projectId, name: file.name };
    e.dataTransfer.setData(FILE_DRAG_TYPE, JSON.stringify(payload));
    e.dataTransfer.effectAllowed = 'copyMove';
    setDragging(true);
  };

  return (
    <li
      className={`file-card${dragging ? ' is-dragging' : ''}`}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(contextMenuAnchor(e));
      }}
      onDragStart={onDragStart}
      onDragEnd={() => setDragging(false)}
    >
      <Link
        to={fileUrl(file)}
        className="file-card-link"
        onKeyDown={(e) => {
          if (!editable) return;
          if (e.key === 'F2') {
            e.preventDefault();
            onRename();
          } else if (e.key === 'Delete') {
            e.preventDefault();
            onDelete();
          }
        }}
      >
        <div className="file-card-thumb">
          <Thumbnail file={file} />
          {file.kind === 'painting' && (
            <span className="file-card-kind" title="Painting">
              <Brush size={12} aria-label="Painting" />
            </span>
          )}
        </div>
        <div className="file-card-meta">
          <span className="file-card-name" title={file.name}>
            {file.name}
          </span>
          <span className="file-card-sub">
            {showProject && (
              <>
                <span className="file-card-project">{file.projectName}</span>
                <span aria-hidden="true"> · </span>
              </>
            )}
            <TimeAgo date={file.updatedAt} prefix="Edited" />
          </span>
        </div>
      </Link>
      <button
        type="button"
        className="btn btn-ghost btn-icon btn-sm file-card-more"
        aria-label={`Actions for ${file.name}`}
        aria-haspopup="menu"
        onClick={(e) => onMenu(e.currentTarget)}
      >
        <Ellipsis size={16} />
      </button>
      {file.role === 'viewer' && <span className="badge file-card-badge">View only</span>}
    </li>
  );
}

function Thumbnail({ file }: { file: FileInfo }) {
  const src = api.files.thumbnailUrl(file.id, file.updatedAt);
  const [failedSrc, setFailedSrc] = useState<string | null>(null);
  if (failedSrc === src) {
    return (
      <span className="file-card-thumb-fallback" aria-hidden="true">
        <FileImage size={28} strokeWidth={1.5} />
      </span>
    );
  }
  return (
    <img
      src={src}
      alt=""
      loading="lazy"
      decoding="async"
      draggable={false}
      onError={() => setFailedSrc(src)}
    />
  );
}

export function FileGridSkeleton({ count = 8 }: { count?: number }) {
  return (
    <ul className="file-grid" aria-busy="true" aria-label="Loading files">
      {Array.from({ length: count }, (_, i) => (
        <li key={i} className="file-card file-card-skeleton">
          <div className="file-card-thumb skeleton" />
          <div className="file-card-meta">
            <span className="skeleton skeleton-text" style={{ width: '70%' }} />
            <span className="skeleton skeleton-text" style={{ width: '40%' }} />
          </div>
        </li>
      ))}
    </ul>
  );
}
