import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Navigate, useLocation } from 'react-router-dom';
import { api, errorMessage, isApiError, onUnauthorized, type SetupInfo, type User } from './api';
import { FullScreenLoader, FullScreenMessage } from '../components/StatusScreens';

export interface AuthContextValue {
  /** The logged-in user, or null when logged out. */
  user: User | null;
  /** True only while the initial session check is running. */
  loading: boolean;
  /** Instance info from `GET /api/setup` (null if it could not be loaded). */
  setup: SetupInfo | null;
  /** Set when the server could not be reached during the session check. */
  error: string | null;
  /** Re-check the session (and setup info). Resolves to the current user. */
  refresh: () => Promise<User | null>;
  logout: () => Promise<void>;
  /** Replace the cached user, e.g. after login or a profile update. */
  setUser: (user: User | null) => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [setup, setSetup] = useState<SetupInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async (): Promise<User | null> => {
    const [setupResult, meResult] = await Promise.allSettled([api.auth.setup(), api.auth.me()]);
    if (setupResult.status === 'fulfilled') setSetup(setupResult.value);

    let next: User | null = null;
    let nextError: string | null = null;
    if (meResult.status === 'fulfilled') {
      next = meResult.value;
    } else if (!isApiError(meResult.reason, 401)) {
      nextError = errorMessage(meResult.reason, 'Could not reach the Flexma server.');
    }
    setUser(next);
    setError(nextError);
    setLoading(false);
    return next;
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A 401 from any regular endpoint: re-check the session (debounced). If it
  // is really gone, `user` becomes null and <RequireAuth> redirects to login.
  const recheckTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    const unsubscribe = onUnauthorized(() => {
      window.clearTimeout(recheckTimer.current);
      recheckTimer.current = window.setTimeout(() => void refresh(), 150);
    });
    return () => {
      unsubscribe();
      window.clearTimeout(recheckTimer.current);
    };
  }, [refresh]);

  const logout = useCallback(async () => {
    try {
      await api.auth.logout();
    } catch {
      // the session is gone either way from the user's point of view
    }
    setUser(null);
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({ user, loading, setup, error, refresh, logout, setUser }),
    [user, loading, setup, error, refresh, logout],
  );

  return <AuthContext value={value}>{children}</AuthContext>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used inside <AuthProvider>');
  return ctx;
}

/** Like useAuth, but for screens that only render when logged in. */
export function useUser(): User {
  const { user } = useAuth();
  if (!user) throw new Error('useUser must be used inside <RequireAuth>');
  return user;
}

/**
 * Renders children only for logged-in users. Otherwise redirects to /setup
 * (fresh instance) or /login?next=<current path>.
 */
export function RequireAuth({ children }: { children: ReactNode }) {
  const { user, loading, setup, error, refresh } = useAuth();
  const location = useLocation();

  if (loading) return <FullScreenLoader />;
  if (!user) {
    if (error) {
      return (
        <FullScreenMessage
          title="Can't reach Flexma"
          message={error}
          action={{ label: 'Try again', onClick: () => void refresh() }}
        />
      );
    }
    if (setup?.needsSetup) return <Navigate to="/setup" replace />;
    const next = location.pathname + location.search;
    const to = next && next !== '/' ? `/login?next=${encodeURIComponent(next)}` : '/login';
    return <Navigate to={to} replace />;
  }
  return children;
}

/** Only allow same-origin relative paths as post-login redirects. */
export function safeNextPath(next: string | null | undefined, fallback = '/'): string {
  if (!next || !next.startsWith('/') || next.startsWith('//') || next.startsWith('/\\')) return fallback;
  return next;
}
