import type { ReactNode } from 'react';

interface EmptyStateProps {
  icon?: ReactNode;
  title: string;
  description?: ReactNode;
  action?: ReactNode;
  compact?: boolean;
}

export function EmptyState({ icon, title, description, action, compact }: EmptyStateProps) {
  return (
    <div className={`empty-state${compact ? ' empty-state-compact' : ''}`}>
      {icon && (
        <div className="empty-state-icon" aria-hidden="true">
          {icon}
        </div>
      )}
      <h3 className="empty-state-title">{title}</h3>
      {description && <p className="empty-state-description">{description}</p>}
      {action && <div className="empty-state-action">{action}</div>}
    </div>
  );
}

interface ErrorStateProps {
  error: unknown;
  onRetry?: () => void;
  title?: string;
}

export function ErrorState({ error, onRetry, title = "Couldn't load this" }: ErrorStateProps) {
  const message = error instanceof Error ? error.message : 'Something went wrong.';
  return (
    <EmptyState
      title={title}
      description={message}
      action={
        onRetry && (
          <button type="button" className="btn" onClick={onRetry}>
            Try again
          </button>
        )
      }
    />
  );
}
