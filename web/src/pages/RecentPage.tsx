import { useState } from 'react';
import { Link } from 'react-router-dom';
import { Bot, PenTool } from 'lucide-react';
import { api } from '../lib/api';
import { useFilesChanged, useWorkspace } from '../lib/dashboard';
import { canEdit } from '../lib/format';
import { useAsync } from '../lib/useAsync';
import { useDocumentTitle } from '../lib/useDocumentTitle';
import { EmptyState } from '../components/EmptyState';
import { FileGrid } from '../components/dashboard/FileGrid';
import { NewFileButton, PageHeader, SearchInput } from '../components/dashboard/PageHeader';
import { useDashboardActions } from '../components/dashboard/useDashboardActions';

export default function RecentPage() {
  const workspace = useWorkspace();
  const { createFile } = useDashboardActions();
  const files = useAsync(() => api.files.listRecent(workspace.id), workspace.id);
  useFilesChanged(files.reload);
  const [filter, setFilter] = useState('');
  const editable = canEdit(workspace.role);
  useDocumentTitle(`Recent · ${workspace.name}`);

  return (
    <div className="page">
      <PageHeader
        title="Recent"
        subtitle={`Recently edited files in ${workspace.name}`}
        actions={
          <>
            {files.data && files.data.length > 0 && <SearchInput value={filter} onChange={setFilter} />}
            {editable && <NewFileButton onCreate={(kind) => createFile(undefined, kind)} />}
          </>
        }
      />
      <FileGrid
        files={files.data}
        loading={files.loading}
        error={files.error}
        onRetry={() => void files.reload()}
        filter={filter}
        showProject
        empty={
          <EmptyState
            icon={<PenTool size={24} />}
            title="No design files yet"
            description={
              editable
                ? 'Create your first design file. Everyone in this workspace can jump in and edit with you in real time.'
                : 'Files shared in this workspace will show up here.'
            }
            action={
              editable && (
                <div className="row row-center">
                  <NewFileButton onCreate={(kind) => createFile(undefined, kind)} />
                  <Link to="/agents" className="btn">
                    <Bot size={16} aria-hidden="true" />
                    <span>Connect an AI agent</span>
                  </Link>
                </div>
              )
            }
          />
        }
      />
    </div>
  );
}
