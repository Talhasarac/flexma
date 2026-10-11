import { initials, readableTextColor } from '../lib/format';

interface AvatarProps {
  name: string;
  /** Background color (user color). */
  color: string;
  size?: number;
  /** Rounded square instead of a circle (used for workspaces). */
  square?: boolean;
  title?: string;
  className?: string;
}

/** First letter of a name, for the toast avatar. */
function initialOf(name: string): string {
  const clean = name.trim().replace(/@.*$/, '');
  const ch = [...clean].find((c) => /\p{L}|\p{N}/u.test(c));
  return (ch ?? '?').toLocaleUpperCase();
}

/**
 * People get a slice of silly toast in their own color with their initial on
 * it (the mark of Silly Toast Studio); workspaces (`square`) get initials on a
 * rounded square.
 */
export function Avatar({ name, color, size = 24, square = false, title, className }: AvatarProps) {
  const a11y = {
    title,
    'aria-hidden': title ? undefined : true,
    role: title ? 'img' : undefined,
    'aria-label': title,
  } as const;
  if (square) {
    return (
      <span
        className={`avatar avatar-square${className ? ` ${className}` : ''}`}
        style={{
          width: size,
          height: size,
          fontSize: Math.max(9, Math.round(size * 0.42)),
          background: color,
          color: readableTextColor(color),
        }}
        {...a11y}
      >
        {initials(name)}
      </span>
    );
  }
  const stroke = size < 20 ? 5 : 4;
  return (
    <span className={`avatar avatar-toast${className ? ` ${className}` : ''}`} style={{ width: size, height: size }} {...a11y}>
      <svg viewBox="3 4 60 52" width={size} height={size} aria-hidden="true">
        <path
          d="M15 27C5 17 23 9 33 9s26 8 16 18l2 24H16z"
          fill={color}
          stroke="#26211D"
          strokeWidth={stroke}
          strokeLinejoin="round"
        />
        <text
          x="33.5"
          y="31.5"
          textAnchor="middle"
          dominantBaseline="central"
          fontSize="24"
          fontWeight="800"
          fill={readableTextColor(color)}
          fontFamily="var(--font-sans)"
        >
          {initialOf(name)}
        </text>
      </svg>
    </span>
  );
}

interface AvatarStackProps {
  people: { id: string; name: string; color: string }[];
  max?: number;
  size?: number;
}

export function AvatarStack({ people, max = 4, size = 24 }: AvatarStackProps) {
  const shown = people.slice(0, max);
  const extra = people.length - shown.length;
  return (
    <span className="avatar-stack">
      {shown.map((p) => (
        <Avatar key={p.id} name={p.name} color={p.color} size={size} title={p.name} />
      ))}
      {extra > 0 && (
        <span className="avatar avatar-more" style={{ width: size, height: size, fontSize: Math.round(size * 0.4) }}>
          +{extra}
        </span>
      )}
    </span>
  );
}
