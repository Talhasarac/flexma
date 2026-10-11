import { lazy, Suspense, type ReactNode } from 'react';
import { BrowserRouter, Route, Routes, useLocation } from 'react-router-dom';
import { AuthProvider, RequireAuth } from './lib/auth';
import { loadEditor } from './lib/preload';
import { DialogProvider } from './components/Dialogs';
import { ErrorBoundary } from './components/ErrorBoundary';
import { FullScreenLoader } from './components/StatusScreens';
import { ToastProvider } from './components/Toast';
import DashboardLayout from './components/dashboard/DashboardLayout';
import AgentsPage from './pages/agents/AgentsPage';
import AdminPage from './pages/admin/AdminPage';
import InvitePage from './pages/auth/InvitePage';
import LoginPage from './pages/auth/LoginPage';
import SetupPage from './pages/auth/SetupPage';
import SignupPage from './pages/auth/SignupPage';
import HomeRedirect from './pages/HomeRedirect';
import NotFoundPage from './pages/NotFoundPage';
import ProfileSettingsPage from './pages/ProfileSettingsPage';
import ProjectPage from './pages/ProjectPage';
import RecentPage from './pages/RecentPage';
import WorkspaceSettingsPage from './pages/workspaceSettings/WorkspaceSettingsPage';

const EditorPage = lazy(loadEditor);
const SharePage = lazy(() => import('./editor/SharePage'));

export default function App() {
  return (
    <BrowserRouter>
      <ToastProvider>
        <AuthProvider>
          <DialogProvider>
            <RoutedErrorBoundary>
              <AppRoutes />
            </RoutedErrorBoundary>
          </DialogProvider>
        </AuthProvider>
      </ToastProvider>
    </BrowserRouter>
  );
}

function RoutedErrorBoundary({ children }: { children: ReactNode }) {
  const { pathname } = useLocation();
  return <ErrorBoundary resetKey={pathname}>{children}</ErrorBoundary>;
}

function AppRoutes() {
  return (
    <Routes>
      {/* Public */}
      <Route path="/setup" element={<SetupPage />} />
      <Route path="/login" element={<LoginPage />} />
      <Route path="/signup" element={<SignupPage />} />
      <Route path="/invite/:token" element={<InvitePage />} />
      <Route
        path="/s/:token"
        element={
          <Suspense fallback={<FullScreenLoader label="Opening shared file" />}>
            <SharePage />
          </Suspense>
        }
      />

      {/* Signed in */}
      <Route
        path="/"
        element={
          <RequireAuth>
            <HomeRedirect />
          </RequireAuth>
        }
      />
      <Route
        element={
          <RequireAuth>
            <DashboardLayout />
          </RequireAuth>
        }
      >
        <Route path="/w/:workspaceId" element={<RecentPage />} />
        <Route path="/w/:workspaceId/p/:projectId" element={<ProjectPage />} />
        <Route path="/w/:workspaceId/settings" element={<WorkspaceSettingsPage />} />
        <Route path="/settings" element={<ProfileSettingsPage />} />
        <Route path="/agents" element={<AgentsPage />} />
        <Route path="/admin" element={<AdminPage />} />
      </Route>

      {/* Full-screen editor, no dashboard chrome */}
      <Route
        path="/file/:fileId"
        element={
          <RequireAuth>
            <Suspense fallback={<FullScreenLoader label="Opening file" />}>
              <EditorPage />
            </Suspense>
          </RequireAuth>
        }
      />

      <Route path="*" element={<NotFoundPage />} />
    </Routes>
  );
}
