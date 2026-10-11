import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { Check } from 'lucide-react';

export interface MenuItem {
  type?: 'item';
  label: ReactNode;
  icon?: ReactNode;
  /** Right-aligned hint, e.g. a keyboard shortcut. */
  hint?: ReactNode;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
  /** Render a check mark (radio-like menus). `false` reserves the space. */
  checked?: boolean;
}

export type MenuEntry = MenuItem | { type: 'separator' } | { type: 'header'; label: ReactNode };

/** Either a point (context menus) or an element to drop down from. */
export type MenuAnchor = { x: number; y: number } | HTMLElement;

export interface MenuProps {
  anchor: MenuAnchor;
  items: MenuEntry[];
  onClose: () => void;
  /** Horizontal alignment relative to an element anchor. */
  align?: 'start' | 'end';
  /** Accessible label for the menu. */
  label?: string;
  minWidth?: number;
}

const MARGIN = 8;

export function Menu({ anchor, items, onClose, align = 'start', label, minWidth = 180 }: MenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const restoreTo = useRef<HTMLElement | null>(
    anchor instanceof HTMLElement
      ? anchor
      : document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null,
  );

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left: number;
    let top: number;
    if (anchor instanceof HTMLElement) {
      const r = anchor.getBoundingClientRect();
      left = align === 'end' ? r.right - width : r.left;
      top = r.bottom + 4;
      if (top + height > vh - MARGIN && r.top - 4 - height >= MARGIN) top = r.top - 4 - height;
    } else {
      left = anchor.x;
      top = anchor.y;
      if (left + width > vw - MARGIN) left = anchor.x - width;
      if (top + height > vh - MARGIN) top = anchor.y - height;
    }
    left = Math.max(MARGIN, Math.min(left, vw - width - MARGIN));
    top = Math.max(MARGIN, Math.min(top, vh - height - MARGIN));
    setPos({ left, top });
  }, [anchor, align]);

  const focusableItems = () =>
    Array.from(ref.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not([disabled])') ?? []);

  // Focus the first item once positioned.
  useEffect(() => {
    if (pos) focusableItems()[0]?.focus({ preventScroll: true });
  }, [pos]);

  const close = useCallback(
    (restoreFocus: boolean) => {
      onClose();
      if (restoreFocus && restoreTo.current?.isConnected) restoreTo.current.focus({ preventScroll: true });
    },
    [onClose],
  );

  // Dismiss on outside press, scroll, resize, blur.
  useEffect(() => {
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (ref.current?.contains(target)) return;
      if (anchor instanceof HTMLElement && anchor.contains(target)) return; // trigger toggles itself
      close(false);
    };
    const onScroll = (e: Event) => {
      if (e.target instanceof Node && ref.current?.contains(e.target)) return;
      close(false);
    };
    const onDismiss = () => close(false);
    document.addEventListener('pointerdown', onPointerDown, true);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('resize', onDismiss);
    window.addEventListener('blur', onDismiss);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown, true);
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', onDismiss);
      window.removeEventListener('blur', onDismiss);
    };
  }, [anchor, close]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const list = focusableItems();
    const index = list.indexOf(document.activeElement as HTMLButtonElement);
    const move = (i: number) => list[(i + list.length) % list.length]?.focus();
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        move(index + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        move(index < 0 ? -1 : index - 1);
        break;
      case 'Home':
        e.preventDefault();
        move(0);
        break;
      case 'End':
        e.preventDefault();
        move(-1);
        break;
      case 'Escape':
        e.preventDefault();
        e.stopPropagation();
        close(true);
        break;
      case 'Tab':
        e.preventDefault();
        close(true);
        break;
    }
  };

  const hasChecks = items.some((i) => (i.type === undefined || i.type === 'item') && i.checked !== undefined);

  return createPortal(
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      tabIndex={-1}
      style={{
        left: pos?.left ?? 0,
        top: pos?.top ?? 0,
        minWidth,
        visibility: pos ? 'visible' : 'hidden',
      }}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((entry, i) => {
        if (entry.type === 'separator') return <div key={i} className="menu-separator" role="separator" />;
        if (entry.type === 'header')
          return (
            <div key={i} className="menu-header" role="presentation">
              {entry.label}
            </div>
          );
        return (
          <button
            key={i}
            type="button"
            role="menuitem"
            className={`menu-item${entry.danger ? ' menu-item-danger' : ''}`}
            disabled={entry.disabled}
            onMouseEnter={(e) => e.currentTarget.focus({ preventScroll: true })}
            onClick={() => {
              close(true);
              entry.onSelect();
            }}
          >
            {hasChecks && (
              <span className="menu-item-check" aria-hidden="true">
                {entry.checked && <Check size={14} />}
              </span>
            )}
            {entry.icon && (
              <span className="menu-item-icon" aria-hidden="true">
                {entry.icon}
              </span>
            )}
            <span className="menu-item-label">{entry.label}</span>
            {entry.hint && <span className="menu-item-hint">{entry.hint}</span>}
          </button>
        );
      })}
    </div>,
    document.body,
  );
}

export interface MenuOpenOptions {
  align?: 'start' | 'end';
  label?: string;
  /** Minimum width in px; `'anchor'` matches the trigger element's width. */
  minWidth?: number | 'anchor';
}

export interface MenuController {
  /** Open at an element (dropdown) or a point (context menu). Opening again with the same element toggles. */
  open: (anchor: MenuAnchor, items: MenuEntry[], opts?: MenuOpenOptions) => void;
  close: () => void;
  isOpen: boolean;
  /** Render this somewhere in your component. */
  element: ReactNode;
}

/** Imperative menu state: `const menu = useMenu(); menu.open(e.currentTarget, items); …{menu.element}`. */
export function useMenu(): MenuController {
  const [state, setState] = useState<({ anchor: MenuAnchor; items: MenuEntry[] } & MenuOpenOptions) | null>(null);

  const close = useCallback(() => setState(null), []);
  const open = useCallback<MenuController['open']>((anchor, items, opts) => {
    setState((prev) =>
      prev && anchor instanceof HTMLElement && prev.anchor === anchor ? null : { anchor, items, ...opts },
    );
  }, []);

  const element = useMemo(
    () =>
      state ? (
        <Menu
          anchor={state.anchor}
          items={state.items}
          align={state.align}
          label={state.label}
          minWidth={
            state.minWidth === 'anchor'
              ? state.anchor instanceof HTMLElement
                ? state.anchor.getBoundingClientRect().width
                : undefined
              : state.minWidth
          }
          onClose={close}
        />
      ) : null,
    [state, close],
  );

  return { open, close, isOpen: state !== null, element };
}

/** Anchor for a `contextmenu` event (keyboard-invoked menus have no pointer position). */
export function contextMenuAnchor(e: ReactMouseEvent<HTMLElement>): MenuAnchor {
  if (e.clientX === 0 && e.clientY === 0) {
    const r = e.currentTarget.getBoundingClientRect();
    return { x: r.left + 12, y: r.top + 12 };
  }
  return { x: e.clientX, y: e.clientY };
}
