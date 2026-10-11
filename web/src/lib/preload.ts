/** Lazy loader for the editor chunk (shared by the route and idle-time preloading). */
export const loadEditor = () => import('../editor/EditorPage');

let preloaded = false;

/** Warm the editor chunk in the background so opening a file feels instant. */
export function preloadEditorWhenIdle(): () => void {
  if (preloaded) return () => {};
  const run = () => {
    preloaded = true;
    loadEditor().catch(() => {
      preloaded = false;
    });
  };
  const id = window.setTimeout(run, 1500);
  return () => window.clearTimeout(id);
}
