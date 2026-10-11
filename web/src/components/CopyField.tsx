import { useEffect, useRef, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import { copyText } from '../lib/clipboard';
import { useToast } from './Toast';

interface CopyButtonProps {
  text: string;
  label?: string;
  /** Toast message after copying. */
  toastMessage?: string;
  className?: string;
  /** Icon-only button. */
  iconOnly?: boolean;
}

export function CopyButton({ text, label = 'Copy', toastMessage = 'Copied to clipboard', className, iconOnly }: CopyButtonProps) {
  const toast = useToast();
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const onClick = async () => {
    const ok = await copyText(text);
    if (!ok) {
      toast.error('Could not copy. Select the text and copy it manually.');
      return;
    }
    setCopied(true);
    if (toastMessage) toast.success(toastMessage, { id: 'copied' });
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), 1600);
  };

  const Icon = copied ? Check : Copy;
  return (
    <button
      type="button"
      className={`btn btn-sm${iconOnly ? ' btn-icon' : ''}${className ? ` ${className}` : ''}`}
      onClick={() => void onClick()}
      aria-label={iconOnly ? label : undefined}
      title={iconOnly ? label : undefined}
    >
      <Icon size={14} aria-hidden="true" />
      {!iconOnly && <span>{copied ? 'Copied' : label}</span>}
    </button>
  );
}

interface CopyFieldProps {
  value: string;
  /** Render as a multi-line code block instead of a single-line field. */
  multiline?: boolean;
  label?: string;
  toastMessage?: string;
  /** Highlight (e.g. a one-time secret). */
  emphasis?: boolean;
}

/** Read-only value with a copy button. Click selects the whole value. */
export function CopyField({ value, multiline, label, toastMessage, emphasis }: CopyFieldProps) {
  const selectAll = (el: HTMLElement) => {
    const sel = window.getSelection();
    if (!sel) return;
    const range = document.createRange();
    range.selectNodeContents(el);
    sel.removeAllRanges();
    sel.addRange(range);
  };

  return (
    <div className={`copy-field${multiline ? ' copy-field-multiline' : ''}${emphasis ? ' copy-field-emphasis' : ''}`}>
      {label && <div className="copy-field-label">{label}</div>}
      <div className="copy-field-row">
        <pre className="copy-field-value" tabIndex={0} onClick={(e) => selectAll(e.currentTarget)}>
          <code>{value}</code>
        </pre>
        <CopyButton text={value} toastMessage={toastMessage} iconOnly={!multiline} className="copy-field-button" />
      </div>
    </div>
  );
}
