import { OS_LABELS, type OS } from '../lib/platform';

const ORDER: OS[] = ['mac', 'windows', 'linux'];

/** macOS / Windows / Linux picker for setup commands. */
export function OsSwitch({ value, onChange }: { value: OS; onChange: (os: OS) => void }) {
  return (
    <div className="segmented" role="radiogroup" aria-label="Computer you're setting up">
      {ORDER.map((os) => (
        <button
          key={os}
          type="button"
          role="radio"
          aria-checked={value === os}
          className={`segmented-item${value === os ? ' is-active' : ''}`}
          onClick={() => onChange(os)}
        >
          {OS_LABELS[os]}
        </button>
      ))}
    </div>
  );
}
