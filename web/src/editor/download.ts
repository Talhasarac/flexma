// Saving exports to the user's disk.

import type { ToastApi } from '../components/Toast';
import { fetchBlob } from '../lib/api';

export function download(name: string, blob: Blob) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function safeName(s: string) {
  return s.replace(/[^\w\- ]+/g, '').trim() || 'export';
}

/** Render PNGs on the server and save them, with progress and failures as toasts. */
export async function exportPngs(toast: ToastApi, items: { url: string; name: string }[]): Promise<void> {
  const id = toast.info(items.length > 1 ? `Exporting ${items.length} PNGs…` : 'Exporting PNG…', { duration: 0 });
  try {
    for (const it of items) download(it.name, await fetchBlob(it.url));
  } catch (err) {
    toast.error(err);
  } finally {
    toast.dismiss(id);
  }
}
