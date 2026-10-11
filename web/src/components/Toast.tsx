import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { CircleAlert, CircleCheck, Info, X } from 'lucide-react';
import { errorMessage } from '../lib/api';

export type ToastKind = 'info' | 'success' | 'error';

export interface ToastOptions {
  kind?: ToastKind;
  /** Milliseconds before auto-dismiss; 0 keeps it until dismissed. */
  duration?: number;
  /** Optional action button, e.g. "Undo". Clicking it also dismisses the toast. */
  action?: { label: string; onClick: () => void };
  /** Reuse an id to replace an existing toast in place (e.g. "Saving…" → "Saved"). */
  id?: string;
}

export interface ToastApi {
  show: (message: ReactNode, opts?: ToastOptions) => string;
  info: (message: ReactNode, opts?: Omit<ToastOptions, 'kind'>) => string;
  success: (message: ReactNode, opts?: Omit<ToastOptions, 'kind'>) => string;
  /** Accepts a message or any thrown value (uses its message). */
  error: (messageOrError: unknown, opts?: Omit<ToastOptions, 'kind'>) => string;
  dismiss: (id: string) => void;
}

interface ToastItem {
  id: string;
  message: ReactNode;
  kind: ToastKind;
  action?: ToastOptions['action'];
}

const ToastContext = createContext<ToastApi | null>(null);
const MAX_TOASTS = 4;
let counter = 0;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const timers = useRef(new Map<string, number>());

  const dismiss = useCallback((id: string) => {
    window.clearTimeout(timers.current.get(id));
    timers.current.delete(id);
    setToasts((list) => list.filter((t) => t.id !== id));
  }, []);

  const show = useCallback(
    (message: ReactNode, opts: ToastOptions = {}): string => {
      const id = opts.id ?? `toast-${++counter}`;
      const kind = opts.kind ?? 'info';
      const item: ToastItem = { id, message, kind, action: opts.action };
      setToasts((list) => {
        const without = list.filter((t) => t.id !== id);
        return [...without, item].slice(-MAX_TOASTS);
      });
      window.clearTimeout(timers.current.get(id));
      const duration = opts.duration ?? (kind === 'error' ? 6000 : 3500);
      if (duration > 0) timers.current.set(id, window.setTimeout(() => dismiss(id), duration));
      return id;
    },
    [dismiss],
  );

  useEffect(() => {
    const map = timers.current;
    return () => map.forEach((t) => window.clearTimeout(t));
  }, []);

  const api = useMemo<ToastApi>(
    () => ({
      show,
      dismiss,
      info: (message, opts) => show(message, { ...opts, kind: 'info' }),
      success: (message, opts) => show(message, { ...opts, kind: 'success' }),
      error: (messageOrError, opts) => {
        const message =
          typeof messageOrError === 'string' ? messageOrError : errorMessage(messageOrError);
        return show(message, { ...opts, kind: 'error' });
      },
    }),
    [show, dismiss],
  );

  return (
    <ToastContext value={api}>
      {children}
      {createPortal(
        <div className="toast-region" aria-live="polite" aria-relevant="additions">
          {toasts.map((t) => (
            <Toast key={t.id} item={t} onDismiss={() => dismiss(t.id)} />
          ))}
        </div>,
        document.body,
      )}
    </ToastContext>
  );
}

function Toast({ item, onDismiss }: { item: ToastItem; onDismiss: () => void }) {
  const Icon = item.kind === 'error' ? CircleAlert : item.kind === 'success' ? CircleCheck : Info;
  return (
    <div className={`toast toast-${item.kind}`} role={item.kind === 'error' ? 'alert' : 'status'}>
      <Icon size={16} className="toast-icon" aria-hidden="true" />
      <div className="toast-message">{item.message}</div>
      {item.action && (
        <button
          type="button"
          className="toast-action"
          onClick={() => {
            item.action?.onClick();
            onDismiss();
          }}
        >
          {item.action.label}
        </button>
      )}
      <button type="button" className="toast-close" aria-label="Dismiss" onClick={onDismiss}>
        <X size={14} />
      </button>
    </div>
  );
}

/** Toast notifications: `const toast = useToast(); toast.success('Saved')`. */
export function useToast(): ToastApi {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error('useToast must be used inside <ToastProvider>');
  return ctx;
}
