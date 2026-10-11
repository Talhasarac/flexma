interface LogoProps {
  size?: number;
  /** Also render the "Flexma" wordmark. */
  wordmark?: boolean;
}

export function Logo({ size = 24, wordmark = false }: LogoProps) {
  return (
    <span className="logo" aria-label={wordmark ? undefined : 'Flexma'}>
      <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
        <rect width="32" height="32" rx="8" fill="var(--accent)" />
        <path d="M10 8h12v4h-8v3h7v4h-7v5h-4z" fill="var(--accent-contrast)" />
      </svg>
      {wordmark && <span className="logo-wordmark">Flexma</span>}
    </span>
  );
}
