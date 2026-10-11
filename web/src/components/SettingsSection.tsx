import type { ReactNode } from 'react';

interface SettingsSectionProps {
  title: ReactNode;
  description?: ReactNode;
  /** Right-aligned header content (e.g. a count or a button). */
  aside?: ReactNode;
  children: ReactNode;
  tone?: 'default' | 'danger';
  id?: string;
}

/** A titled card used on settings pages. */
export function SettingsSection({ title, description, aside, children, tone = 'default', id }: SettingsSectionProps) {
  const headingId = id ? `${id}-title` : undefined;
  return (
    <section className={`card settings-section${tone === 'danger' ? ' settings-section-danger' : ''}`} aria-labelledby={headingId} id={id}>
      <div className="settings-section-header">
        <div>
          <h2 className="settings-section-title" id={headingId}>
            {title}
          </h2>
          {description && <p className="settings-section-description">{description}</p>}
        </div>
        {aside && <div className="settings-section-aside">{aside}</div>}
      </div>
      <div className="settings-section-body">{children}</div>
    </section>
  );
}
