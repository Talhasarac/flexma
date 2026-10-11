import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { Logo } from './Logo';

export function Spinner({ size = 16, label = 'Loading' }: { size?: number; label?: string }) {
  return (
    <span
      className="spinner"
      role="status"
      aria-label={label}
      style={{ width: size, height: size }}
    />
  );
}

/** Centered spinner covering the viewport (route-level loading). */
export function FullScreenLoader({ label = 'Loading' }: { label?: string }) {
  return (
    <div className="fullscreen-center" aria-busy="true">
      <Spinner size={24} label={label} />
    </div>
  );
}

interface FullScreenMessageProps {
  title: string;
  message?: ReactNode;
  action?: { label: string; onClick: () => void };
  /** Show a "Back to Flexma" link. */
  homeLink?: boolean;
}

export function FullScreenMessage({ title, message, action, homeLink }: FullScreenMessageProps) {
  return (
    <div className="fullscreen-center">
      <div className="status-card">
        <Logo size={28} />
        <h1 className="status-card-title">{title}</h1>
        {message && <p className="status-card-message">{message}</p>}
        <div className="status-card-actions">
          {action && (
            <button type="button" className="btn btn-primary" onClick={action.onClick}>
              {action.label}
            </button>
          )}
          {homeLink && (
            <Link to="/" className="btn">
              Back to Flexma
            </Link>
          )}
        </div>
      </div>
    </div>
  );
}
