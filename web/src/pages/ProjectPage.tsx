import { useMemo, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { Ellipsis, FolderOpen, PencilLine, SearchX, Trash } from 'lucide-react';
import { api, type FileInfo } from '../lib/api';
import { useDashboard, useFilesChanged, useWorkspace } from '../lib/dashboard';
import { canEdit, pluralize } from '../lib/format';
import { useAsync } from '../lib/useAsync';
import { useDocumentTitle } from '../lib/useDocumentTitle';
import { EmptyState } from '../components/EmptyState';
import { useMenu } from '../components/Menu';
import { FileGrid } from '../components/dashboard/FileGrid';
import { NewFileButton, PageHeader, SearchInput } from '../components/dashboard/PageHeader';
import { useDashboardActions } from '../components/dashboard/useDashboardActions';

type SortKey = 'updated' | 'name' | 'created';

const SORTERS: Record<SortKey, (a: FileInfo, b: FileInfo) => number> = {
  updated: (a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt),
  created: (a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt),
  name: (a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }),
};

export default function ProjectPage() {
  const { projectId = '' } = useParams();
  const workspace = useWorkspace();
  const { projects } = useDashboard();
  const { createFile, renameProject, deleteProject } = useDashboardActions();
  const menu = useMenu();
  const files = useAsync(() => api.projects.listFiles(projectId), projectId);
  useFilesChanged(files.reload);
  const [filter, setFilter] = useState('');
  const [sort, setSort] = useState<SortKey>('updated');

  const project = projects?.find((p) => p.id === projectId);
  const editable = canEdit(workspace.role);
  useDocumentTitle(project?.name);

  const sorted = useMemo(() => (files.data ? [...files.data].sort(SORTERS[sort]) : undefined), [files.data, sort]);

  if (projects && !project) {
    return (
      <div className="page">
        <EmptyState
          icon={<SearchX size={24} />}
          title="Project not found"
          description="It may have been deleted or moved."
          action={
            <Link to={`/w/${workspace.id}`} className="btn">
              Back to Recent
            </Link>
          }
        />
      </div>
    );
  }

  const count = files.data?.length ?? project?.fileCount;

  return (
    <div className="page">
      <PageHeader
        eyebrow="Project"
        title={
          project ? (
            <span className="row row-tight">
              <span className="truncate">{project.name}</span>
              {editable && (
                <button
                  type="button"
                  className="btn btn-ghost btn-icon btn-sm"
                  aria-label="Project actions"
                  aria-haspopup="menu"
                  onClick={(e) =>
                    menu.open(
                      e.currentTarget,
                      [
                        { label: 'Rename project', icon: <PencilLine size={14} />, onSelect: () => void renameProject(project) },
                        { type: 'separator' },
                        {
                          label: 'Delete project',
                          icon: <Trash size={14} />,
                          danger: true,
                          onSelect: () => void deleteProject(project),
                        },
                      ],
                      { label: 'Project actions' },
                    )
                  }
                >
                  <Ellipsis size={16} />
                </button>
              )}
            </span>
          ) : (
            <span className="skeleton skeleton-title" />
          )
        }
        subtitle={count !== undefined ? pluralize(count, 'file') : undefined}
        actions={
          <>
            {files.data && files.data.length > 0 && (
              <>
                <SearchInput value={filter} onChange={setFilter} />
                <select
                  className="select select-sm"
                  value={sort}
                  aria-label="Sort files"
                  onChange={(e) => setSort(e.target.value as SortKey)}
                >
                  <option value="updated">Last edited</option>
                  <option value="created">Date created</option>
                  <option value="name">Name</option>
                </select>
              </>
            )}
            {editable && <NewFileButton onCreate={() => createFile(projectId)} />}
          </>
        }
      />
      <FileGrid
        files={sorted}
        loading={files.loading}
        error={files.error}
        onRetry={() => void files.reload()}
        filter={filter}
        empty={
          <EmptyState
            icon={<FolderOpen size={24} />}
            title="This project is empty"
            description={
              editable
                ? 'Create a design file here, or drag files from Recent onto this project in the sidebar.'
                : 'No files have been added to this project yet.'
            }
            action={editable && <NewFileButton onCreate={() => createFile(projectId)} />}
          />
        }
      />
      {menu.element}
    </div>
  );
}
