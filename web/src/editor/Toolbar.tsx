// Floating bottom toolbar (Figma UI3 style).
import { useRef, type ReactElement } from 'react';
import { ChevronUp, Circle, Frame, Hand, MessageCircle, Minus, MousePointer2, PenTool, Square, Star, Triangle, Type } from 'lucide-react';
import { useMenu } from '../components/Menu';
import { useCommentMode, useUi, type EditorStore } from './store';
import type { Tool } from './types';

const SHAPES: { tool: Tool; label: string; key: string; icon: ReactElement }[] = [
  { tool: 'rectangle', label: 'Rectangle', key: 'R', icon: <Square size={16} /> },
  { tool: 'ellipse', label: 'Ellipse', key: 'O', icon: <Circle size={16} /> },
  { tool: 'line', label: 'Line', key: 'L', icon: <Minus size={16} /> },
  { tool: 'polygon', label: 'Polygon', key: '', icon: <Triangle size={16} /> },
  { tool: 'star', label: 'Star', key: '', icon: <Star size={16} /> },
];

export function Toolbar({ store }: { store: EditorStore }) {
  const ui = useUi(store);
  const commentMode = useCommentMode(store);
  const menu = useMenu();
  const lastShape = useRef<Tool>('rectangle');
  const set = (t: Tool) => {
    store.setCommentMode(false);
    store.engine.set_tool(t);
    store.poll();
  };
  const shape = SHAPES.find((s) => s.tool === ui.tool) ?? SHAPES.find((s) => s.tool === lastShape.current)!;
  if (SHAPES.some((s) => s.tool === ui.tool)) lastShape.current = ui.tool;

  const btn = (t: Tool, title: string, icon: ReactElement) => (
    <button type="button" className={`tool${ui.tool === t && !commentMode ? ' active' : ''}`} title={title} aria-label={title} aria-pressed={ui.tool === t && !commentMode} onClick={() => set(t)}>
      {icon}
    </button>
  );

  return (
    <div className="toolbar" role="toolbar" aria-label="Tools">
      {btn('select', 'Move (V)', <MousePointer2 size={16} />)}
      {btn('hand', 'Hand (H, or hold Space)', <Hand size={16} />)}
      {ui.canEdit && (
        <>
          <span className="tool-sep" />
          {btn('frame', 'Frame (F)', <Frame size={16} />)}
          <div className={`tool-split${SHAPES.some((s) => s.tool === ui.tool) ? ' active' : ''}`}>
            <button type="button" className="tool" title={`${shape.label}${shape.key ? ` (${shape.key})` : ''}`} onClick={() => set(shape.tool)}>
              {shape.icon}
            </button>
            <button
              type="button"
              className="tool tool-more"
              aria-label="More shapes"
              onClick={(e) =>
                menu.open(
                  e.currentTarget,
                  SHAPES.map((s) => ({ label: s.label, icon: s.icon, hint: s.key, onSelect: () => set(s.tool), checked: ui.tool === s.tool })),
                  { label: 'Shapes' },
                )
              }
            >
              <ChevronUp size={12} />
            </button>
          </div>
          {btn('pen', 'Pen (P)', <PenTool size={16} />)}
          {btn('text', 'Text (T)', <Type size={16} />)}
        </>
      )}
      <span className="tool-sep" />
      <button
        type="button"
        className={`tool${commentMode ? ' active' : ''}`}
        title="Comment (C)"
        aria-label="Comment"
        aria-pressed={commentMode}
        onClick={() => store.setCommentMode(!commentMode)}
      >
        <MessageCircle size={16} />
      </button>
      {!ui.canEdit && <span className="view-only">View only</span>}
      {menu.element}
    </div>
  );
}
