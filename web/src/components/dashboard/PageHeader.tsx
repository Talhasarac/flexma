import { useState, type ReactNode } from 'react';
import { Plus, Search, X } from 'lucide-react';
import { Spinner } from '../StatusScreens';

interface PageHeaderProps {
  title: ReactNode;
  subtitle?: ReactNode;
  /** Small label above the title, e.g. "Project". */
  eyebrow?: ReactNode;
  actions?: ReactNode;
}

export function PageHeader({ title, subtitle, eyebrow, actions }: PageHeaderProps) {
  return (
    <header className="page-header">
      <div className="page-header-text">
        {eyebrow && <div className="page-eyebrow">{eyebrow}</div>}
        <h1 className="page-title">{title}</h1>
        {subtitle && <p className="page-subtitle">{subtitle}</p>}
      </div>
      {actions && <div className="page-header-actions">{actions}</div>}
    </header>
  );
}

export function SearchInput({
  value,
  onChange,
  placeholder = 'Search files',
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  return (
    <div className="search-input">
      <Search size={14} className="search-input-icon" aria-hidden="true" />
      <input
        className="input input-sm"
        type="search"
        value={value}
        placeholder={placeholder}
        aria-label={placeholder}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && value) {
            e.preventDefault();
            onChange('');
          }
        }}
      />
      {value && (
        <button type="button" className="search-input-clear" aria-label="Clear search" onClick={() => onChange('')}>
          <X size={12} />
        </button>
      )}
    </div>
  );
}

/** Primary "New design file" button with a busy state. */
export function NewFileButton({ onCreate, label = 'New design file' }: { onCreate: () => Promise<unknown>; label?: string }) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      type="button"
      className="btn btn-primary"
      disabled={busy}
      onClick={async () => {
        setBusy(true);
        try {
          await onCreate();
        } finally {
          setBusy(false);
        }
      }}
    >
      {busy ? <Spinner size={14} label="Creating" /> : <Plus size={16} aria-hidden="true" />}
      <span>{label}</span>
    </button>
  );
}
