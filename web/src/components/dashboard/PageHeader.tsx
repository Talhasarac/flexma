import { useState, type ReactNode } from 'react';
import { Brush, ChevronDown, PenTool, Plus, Search, X } from 'lucide-react';
import type { FileKind } from '../../lib/api';
import { useMenu } from '../Menu';
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
const KINDS: { kind: FileKind; title: string; description: string; icon: ReactNode }[] = [
  { kind: 'design', title: 'Design file', description: 'Screens, UI and components', icon: <PenTool size={16} /> },
  { kind: 'painting', title: 'Painting', description: 'A canvas with the brush ready, painted together live', icon: <Brush size={16} /> },
];

/** "New file": asks whether it's a design file or a painting. */
export function NewFileButton({ onCreate, label = 'New file' }: { onCreate: (kind: FileKind) => Promise<unknown>; label?: string }) {
  const [busy, setBusy] = useState(false);
  const menu = useMenu();
  const create = async (kind: FileKind) => {
    setBusy(true);
    try {
      await onCreate(kind);
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <button
        type="button"
        className="btn btn-primary"
        disabled={busy}
        aria-haspopup="menu"
        onClick={(e) =>
          menu.open(
            e.currentTarget,
            KINDS.map((k) => ({
              icon: k.icon,
              label: (
                <span className="new-file-kind">
                  <span>{k.title}</span>
                  <span className="text-muted">{k.description}</span>
                </span>
              ),
              onSelect: () => void create(k.kind),
            })),
            { label: 'New file', align: 'end' },
          )
        }
      >
        {busy ? <Spinner size={14} label="Creating" /> : <Plus size={16} aria-hidden="true" />}
        <span>{label}</span>
        <ChevronDown size={14} aria-hidden="true" />
      </button>
      {menu.element}
    </>
  );
}
