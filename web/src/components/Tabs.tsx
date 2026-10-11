import { useId, useRef, type KeyboardEvent, type ReactNode } from 'react';

export interface TabItem<T extends string> {
  id: T;
  label: ReactNode;
  icon?: ReactNode;
}

interface TabsProps<T extends string> {
  tabs: TabItem<T>[];
  value: T;
  onChange: (id: T) => void;
  /** Accessible name of the tab list. */
  label: string;
  /** Render the active panel. */
  children: ReactNode;
}

/** Accessible tabs (arrow keys / Home / End move between tabs). */
export function Tabs<T extends string>({ tabs, value, onChange, label, children }: TabsProps<T>) {
  const baseId = useId();
  const listRef = useRef<HTMLDivElement>(null);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const index = tabs.findIndex((t) => t.id === value);
    let next = -1;
    if (e.key === 'ArrowRight') next = (index + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') next = (index - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    if (next < 0) return;
    e.preventDefault();
    onChange(tabs[next].id);
    listRef.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
  };

  return (
    <div className="tabs-root">
      <div ref={listRef} className="tabs" role="tablist" aria-label={label} onKeyDown={onKeyDown}>
        {tabs.map((t) => {
          const selected = t.id === value;
          return (
            <button
              key={t.id}
              type="button"
              role="tab"
              id={`${baseId}-tab-${t.id}`}
              aria-selected={selected}
              aria-controls={`${baseId}-panel`}
              tabIndex={selected ? 0 : -1}
              className={`tab${selected ? ' is-active' : ''}`}
              onClick={() => onChange(t.id)}
            >
              {t.icon}
              {t.label}
            </button>
          );
        })}
      </div>
      <div
        className="tab-panel"
        role="tabpanel"
        id={`${baseId}-panel`}
        aria-labelledby={`${baseId}-tab-${value}`}
        tabIndex={0}
      >
        {children}
      </div>
    </div>
  );
}
