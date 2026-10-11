import { createContext, useCallback, useContext, useMemo, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Check } from 'lucide-react';
import { Modal } from './Modal';

export interface ConfirmOptions {
  title: string;
  message?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Style the confirm button as destructive. */
  danger?: boolean;
  /** The user must type this exact text before confirming (for very destructive actions). */
  requireText?: string;
}

export interface PromptOptions {
  title: string;
  message?: ReactNode;
  label?: string;
  defaultValue?: string;
  placeholder?: string;
  confirmLabel?: string;
  maxLength?: number;
  /** Return an error message to block submission. Empty values are always rejected. */
  validate?: (value: string) => string | null;
}

export interface ChoiceOption<T extends string> {
  value: T;
  label: string;
  description?: string;
  disabled?: boolean;
}

export interface ChooseOptions<T extends string> {
  title: string;
  message?: ReactNode;
  options: ChoiceOption<T>[];
  defaultValue?: T;
  confirmLabel?: string;
}

export interface DialogsApi {
  /** Resolves true when confirmed. */
  confirm: (opts: ConfirmOptions) => Promise<boolean>;
  /** Resolves to the trimmed value, or null when cancelled. */
  prompt: (opts: PromptOptions) => Promise<string | null>;
  /** Pick one option from a list. Resolves null when cancelled. */
  choose: <T extends string>(opts: ChooseOptions<T>) => Promise<T | null>;
}

type DialogRequest =
  | { kind: 'confirm'; opts: ConfirmOptions; resolve: (v: boolean) => void }
  | { kind: 'prompt'; opts: PromptOptions; resolve: (v: string | null) => void }
  | { kind: 'choose'; opts: ChooseOptions<string>; resolve: (v: string | null) => void };

type ActiveDialog = DialogRequest & { id: number };
let dialogCounter = 0;

const DialogsContext = createContext<DialogsApi | null>(null);

export function DialogProvider({ children }: { children: ReactNode }) {
  const [active, setActive] = useState<ActiveDialog | null>(null);
  const activeRef = useRef<ActiveDialog | null>(null);

  const open = useCallback((request: DialogRequest) => {
    // Only one dialog at a time: cancel whatever was open.
    const prev = activeRef.current;
    if (prev) {
      if (prev.kind === 'confirm') prev.resolve(false);
      else prev.resolve(null);
    }
    const dialog: ActiveDialog = { ...request, id: ++dialogCounter };
    activeRef.current = dialog;
    setActive(dialog);
  }, []);

  const close = useCallback(() => {
    activeRef.current = null;
    setActive(null);
  }, []);

  const api = useMemo<DialogsApi>(
    () => ({
      confirm: (opts) => new Promise<boolean>((resolve) => open({ kind: 'confirm', opts, resolve })),
      prompt: (opts) => new Promise<string | null>((resolve) => open({ kind: 'prompt', opts, resolve })),
      choose: <T extends string>(opts: ChooseOptions<T>) =>
        new Promise<T | null>((resolve) =>
          open({
            kind: 'choose',
            opts: opts as ChooseOptions<string>,
            resolve: resolve as (v: string | null) => void,
          }),
        ),
    }),
    [open],
  );

  const finish = <V,>(resolve: (v: V) => void, value: V) => {
    close();
    resolve(value);
  };

  return (
    <DialogsContext value={api}>
      {children}
      {active?.kind === 'confirm' && (
        <ConfirmDialog
          key={active.id}
          opts={active.opts}
          onDone={(v) => finish(active.resolve, v)}
        />
      )}
      {active?.kind === 'prompt' && (
        <PromptDialog key={active.id} opts={active.opts} onDone={(v) => finish(active.resolve, v)} />
      )}
      {active?.kind === 'choose' && (
        <ChooseDialog key={active.id} opts={active.opts} onDone={(v) => finish(active.resolve, v)} />
      )}
    </DialogsContext>
  );
}

function ConfirmDialog({ opts, onDone }: { opts: ConfirmOptions; onDone: (v: boolean) => void }) {
  const [typed, setTyped] = useState('');
  const blocked = opts.requireText !== undefined && typed.trim() !== opts.requireText.trim();
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!blocked) onDone(true);
  };
  return (
    <Modal
      open
      onClose={() => onDone(false)}
      title={opts.title}
      footer={
        <>
          <button type="button" className="btn" onClick={() => onDone(false)}>
            {opts.cancelLabel ?? 'Cancel'}
          </button>
          <button
            type="submit"
            form="confirm-dialog-form"
            className={`btn ${opts.danger ? 'btn-danger' : 'btn-primary'}`}
            disabled={blocked}
            data-autofocus={opts.requireText === undefined ? true : undefined}
          >
            {opts.confirmLabel ?? 'Confirm'}
          </button>
        </>
      }
    >
      <form id="confirm-dialog-form" onSubmit={submit} className="stack">
        {opts.message && <div className="text-muted">{opts.message}</div>}
        {opts.requireText !== undefined && (
          <label className="field">
            <span className="field-label">
              Type <strong className="text-strong">{opts.requireText}</strong> to confirm
            </span>
            <input
              className="input"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              autoComplete="off"
              spellCheck={false}
              data-autofocus
            />
          </label>
        )}
      </form>
    </Modal>
  );
}

function PromptDialog({ opts, onDone }: { opts: PromptOptions; onDone: (v: string | null) => void }) {
  const [value, setValue] = useState(opts.defaultValue ?? '');
  const [error, setError] = useState<string | null>(null);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const v = value.trim();
    if (!v) {
      setError('Please enter a value.');
      return;
    }
    const problem = opts.validate?.(v) ?? null;
    if (problem) {
      setError(problem);
      return;
    }
    onDone(v);
  };
  return (
    <Modal
      open
      onClose={() => onDone(null)}
      title={opts.title}
      footer={
        <>
          <button type="button" className="btn" onClick={() => onDone(null)}>
            Cancel
          </button>
          <button type="submit" form="prompt-dialog-form" className="btn btn-primary">
            {opts.confirmLabel ?? 'Save'}
          </button>
        </>
      }
    >
      <form id="prompt-dialog-form" onSubmit={submit} className="stack" noValidate>
        {opts.message && <div className="text-muted">{opts.message}</div>}
        <label className="field">
          {opts.label && <span className="field-label">{opts.label}</span>}
          <input
            className={`input${error ? ' input-invalid' : ''}`}
            value={value}
            placeholder={opts.placeholder}
            maxLength={opts.maxLength ?? 200}
            onChange={(e) => {
              setValue(e.target.value);
              setError(null);
            }}
            aria-invalid={error ? true : undefined}
            autoComplete="off"
            data-autofocus
          />
          {error && <span className="field-error">{error}</span>}
        </label>
      </form>
    </Modal>
  );
}

function ChooseDialog({ opts, onDone }: { opts: ChooseOptions<string>; onDone: (v: string | null) => void }) {
  const firstEnabled = opts.options.find((o) => !o.disabled)?.value ?? null;
  const [value, setValue] = useState<string | null>(opts.defaultValue ?? firstEnabled);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (value !== null) onDone(value);
  };
  return (
    <Modal
      open
      onClose={() => onDone(null)}
      title={opts.title}
      footer={
        <>
          <button type="button" className="btn" onClick={() => onDone(null)}>
            Cancel
          </button>
          <button type="submit" form="choose-dialog-form" className="btn btn-primary" disabled={value === null}>
            {opts.confirmLabel ?? 'Done'}
          </button>
        </>
      }
    >
      <form id="choose-dialog-form" onSubmit={submit} className="stack">
        {opts.message && <div className="text-muted">{opts.message}</div>}
        <div className="choice-list" role="radiogroup" aria-label={opts.title}>
          {opts.options.map((o) => (
            <label
              key={o.value}
              className={`choice${value === o.value ? ' is-selected' : ''}${o.disabled ? ' is-disabled' : ''}`}
            >
              <input
                type="radio"
                name="choose-dialog"
                value={o.value}
                checked={value === o.value}
                disabled={o.disabled}
                onChange={() => setValue(o.value)}
                data-autofocus={value === o.value ? true : undefined}
              />
              <span className="choice-text">
                <span className="choice-label">{o.label}</span>
                {o.description && <span className="choice-description">{o.description}</span>}
              </span>
              {value === o.value && <Check size={14} className="choice-check" aria-hidden="true" />}
            </label>
          ))}
        </div>
      </form>
    </Modal>
  );
}

/** Promise-based confirm / prompt / choose dialogs. */
export function useDialogs(): DialogsApi {
  const ctx = useContext(DialogsContext);
  if (!ctx) throw new Error('useDialogs must be used inside <DialogProvider>');
  return ctx;
}
