// In-place text editing: a transparent textarea laid exactly over the text node
// (the canvas hides the node while editing). Each keystroke updates the node,
// so collaborators see typing live.
import { useEffect, useLayoutEffect, useRef } from 'react';
import { useUi, type EditorStore } from './store';

export function TextEditOverlay({ store }: { store: EditorStore }) {
  const ui = useUi(store);
  const t = ui.editingText;
  const ref = useRef<HTMLTextAreaElement>(null);
  const editingId = t?.id ?? null;

  useEffect(() => {
    const el = ref.current;
    if (!el || !editingId) return;
    el.value = store.getUi().editingText?.characters ?? '';
    el.focus();
    el.select();
  }, [editingId, store]);

  // Keep the textarea's value in sync if someone else edits the same text.
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && t && document.activeElement !== el && el.value !== t.characters) el.value = t.characters;
  }, [t]);

  if (!t) return null;
  const z = t.zoom;
  const fixedWidth = t.autoResize !== 'WIDTH_AND_HEIGHT';
  return (
    <textarea
      ref={ref}
      className="text-edit-overlay"
      spellCheck={false}
      style={{
        left: t.x,
        top: t.y,
        width: fixedWidth ? t.width : Math.max(t.width + t.fontSize, 4),
        height: Math.max(t.height, t.lineHeight),
        transform: `scale(${z}) rotate(${t.rotation}deg)`,
        fontFamily: `"${t.fontFamily}", "Inter Variable", Inter, system-ui, sans-serif`,
        fontSize: t.fontSize,
        fontWeight: t.fontWeight,
        fontStyle: t.italic ? 'italic' : 'normal',
        lineHeight: `${t.lineHeight}px`,
        letterSpacing: `${t.letterSpacing}px`,
        textAlign: t.textAlign.toLowerCase() as 'left' | 'center' | 'right',
        color: t.color,
        caretColor: t.color,
        whiteSpace: fixedWidth && t.wrap !== false ? 'pre-wrap' : 'pre',
      }}
      onInput={(e) => {
        store.engine.set_editing_text(e.currentTarget.value);
        store.poll();
      }}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Escape' || (e.key === 'Enter' && (e.metaKey || e.ctrlKey))) {
          e.preventDefault();
          store.engine.end_text_edit();
          store.poll();
        }
      }}
      onBlur={() => {
        if (store.getUi().editingText) {
          store.engine.end_text_edit();
          store.poll();
        }
      }}
    />
  );
}
