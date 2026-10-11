import { useEffect, useState } from 'react';
import { formatDateTime, formatRelativeTime } from '../lib/format';

/** Re-renders every minute so relative times stay fresh. */
function useNow(intervalMs = 60_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}

/** "2h ago" with the full date in a tooltip. */
export function TimeAgo({ date, prefix }: { date: string | null | undefined; prefix?: string }) {
  const now = useNow();
  if (!date) return <span>{prefix ? `${prefix} never` : 'never'}</span>;
  return (
    <time dateTime={date} title={formatDateTime(date)}>
      {prefix ? `${prefix} ` : ''}
      {formatRelativeTime(date, now)}
    </time>
  );
}
