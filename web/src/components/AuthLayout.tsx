import type { ReactNode } from 'react';
import { useAuth } from '../lib/auth';
import { Logo } from './Logo';
import { MadeBy } from './MadeBy';

interface AuthLayoutProps {
  title: ReactNode;
  subtitle?: ReactNode;
  children: ReactNode;
  /** Small print under the card (e.g. "Already have an account?"). */
  footer?: ReactNode;
  wide?: boolean;
}

/** Centered card layout for setup, login, signup and invite screens. */
export function AuthLayout({ title, subtitle, children, footer, wide }: AuthLayoutProps) {
  const { setup } = useAuth();
  const instance = setup?.instanceName && setup.instanceName !== 'Flexma' ? setup.instanceName : null;
  return (
    <div className="auth-screen">
      <div className={`auth-card${wide ? ' auth-card-wide' : ''}`}>
        <div className="auth-brand">
          <Logo size={28} wordmark />
          {instance && <span className="auth-instance">{instance}</span>}
        </div>
        <h1 className="auth-title">{title}</h1>
        {subtitle && <p className="auth-subtitle">{subtitle}</p>}
        <div className="auth-body">{children}</div>
      </div>
      {footer && <div className="auth-footer">{footer}</div>}
      <div className="auth-made-by">
        <MadeBy />
      </div>
    </div>
  );
}

interface FieldProps {
  label: string;
  children: ReactNode;
  hint?: ReactNode;
}

export function Field({ label, children, hint }: FieldProps) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}

export function FormError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <p className="form-error" role="alert">
      {message}
    </p>
  );
}
