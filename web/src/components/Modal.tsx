import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export interface ModalProps {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  /** Buttons row; rendered right-aligned below the body. */
  footer?: ReactNode;
  size?: 'sm' | 'md' | 'lg';
  /** When false, Escape / backdrop clicks are ignored (e.g. while submitting). */
  dismissible?: boolean;
}

let openModals = 0;

/**
 * Accessible modal dialog: focus is moved inside (to `[data-autofocus]` or the
 * first focusable element), trapped while open and restored on close; Escape
 * and backdrop clicks close it.
 */
export function Modal({ open, ...rest }: ModalProps) {
  if (!open) return null;
  return createPortal(<ModalDialog {...rest} />, document.body);
}

function ModalDialog({
  onClose,
  title,
  description,
  children,
  footer,
  size = 'sm',
  dismissible = true,
}: Omit<ModalProps, 'open'>) {
  const titleId = useId();
  const descId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const pressStartedOnBackdrop = useRef(false);

  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const panel = panelRef.current;
    if (panel) {
      const target =
        panel.querySelector<HTMLElement>('[data-autofocus]') ??
        panel.querySelector<HTMLElement>(`.modal-body ${FOCUSABLE}`) ??
        panel.querySelector<HTMLElement>(`.modal-footer ${FOCUSABLE}`) ??
        panel;
      target.focus();
      if (target instanceof HTMLInputElement && target.type === 'text') target.select();
    }
    openModals++;
    document.body.classList.add('modal-open');
    return () => {
      openModals--;
      if (openModals === 0) document.body.classList.remove('modal-open');
      if (previouslyFocused?.isConnected) previouslyFocused.focus();
    };
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      if (dismissible) onClose();
      return;
    }
    if (e.key !== 'Tab' || !panelRef.current) return;
    const items = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => el.offsetParent !== null || el === document.activeElement,
    );
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    if (e.shiftKey && (document.activeElement === first || document.activeElement === panelRef.current)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        pressStartedOnBackdrop.current = e.target === e.currentTarget;
      }}
      onClick={(e) => {
        if (dismissible && pressStartedOnBackdrop.current && e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        className={`modal modal-${size}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descId : undefined}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="modal-header">
          <h2 id={titleId} className="modal-title">
            {title}
          </h2>
          {dismissible && (
            <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Close" onClick={onClose}>
              <X size={16} />
            </button>
          )}
        </div>
        {description && (
          <p id={descId} className="modal-description">
            {description}
          </p>
        )}
        {children && <div className="modal-body">{children}</div>}
        {footer && <div className="modal-footer">{footer}</div>}
      </div>
    </div>
  );
}
