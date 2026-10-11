import { useCallback, useEffect, useRef, useState } from 'react';

export interface AsyncState<T> {
  data: T | undefined;
  error: unknown;
  loading: boolean;
  /** Re-run the loader, keeping the current data visible meanwhile. */
  reload: () => Promise<void>;
  /** Locally replace (or update) the data, e.g. after a mutation. */
  setData: (next: T | ((prev: T | undefined) => T)) => void;
}

/**
 * Runs `load` whenever `key` changes (resetting data), ignoring stale results.
 * `key` should encode every input of `load` (e.g. an id).
 */
export function useAsync<T>(load: () => Promise<T>, key: string): AsyncState<T> {
  const loadRef = useRef(load);
  useEffect(() => {
    loadRef.current = load;
  });

  const requestId = useRef(0);
  const [state, setState] = useState<{ data: T | undefined; error: unknown; loading: boolean }>({
    data: undefined,
    error: undefined,
    loading: true,
  });

  const run = useCallback(async () => {
    const id = ++requestId.current;
    setState((s) => ({ ...s, loading: true }));
    try {
      const data = await loadRef.current();
      if (id === requestId.current) setState({ data, error: undefined, loading: false });
    } catch (error) {
      if (id === requestId.current) setState((s) => ({ data: s.data, error, loading: false }));
    }
  }, []);

  useEffect(() => {
    setState({ data: undefined, error: undefined, loading: true });
    void run();
    return () => {
      requestId.current++;
    };
  }, [key, run]);

  const setData = useCallback((next: T | ((prev: T | undefined) => T)) => {
    setState((s) => ({
      ...s,
      data: typeof next === 'function' ? (next as (prev: T | undefined) => T)(s.data) : next,
    }));
  }, []);

  return { ...state, reload: run, setData };
}
