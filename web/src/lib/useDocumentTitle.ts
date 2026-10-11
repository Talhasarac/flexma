import { useEffect } from 'react';

/** Sets `document.title` to "<title> – Flexma" while mounted. */
export function useDocumentTitle(title: string | null | undefined): void {
  useEffect(() => {
    if (!title) return;
    const previous = document.title;
    document.title = `${title} – Flexma`;
    return () => {
      document.title = previous;
    };
  }, [title]);
}
