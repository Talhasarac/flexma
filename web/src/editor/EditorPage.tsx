// The design editor route (/file/:fileId).
import { useCallback, useEffect, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react';
import { useParams } from 'react-router-dom';
import { api, errorMessage, type FileInfo } from '../lib/api';
import { useToast } from '../components/Toast';
import { useMenu } from '../components/Menu';
import { FullScreenLoader, FullScreenMessage } from '../components/StatusScreens';
import { useDocumentTitle } from '../lib/useDocumentTitle';
import { useAuth } from '../lib/auth';
import { createEditor, loadEngine } from '../engine';
import { EditorStore, useCommentMode, useSelection, useUi } from './store';
import { CommentsPanel } from './CommentsLayer';
import { Canvas } from './Canvas';
import { Toolbar } from './Toolbar';
import { TopBar } from './TopBar';
import { LayersPanel } from './LayersPanel';
import { PropertiesPanel } from './PropertiesPanel';
import './editor.css';

const CLIP_PREFIX = 'flexma-clipboard:';

export default function EditorPage() {
  const { fileId = '' } = useParams();
  const [file, setFile] = useState<FileInfo | null>(null);
  const [store, setStore] = useState<EditorStore | null>(null);
  const userId = useAuth().user?.id ?? null;
  const [canvas] = useState(() => document.createElement('canvas'));
  const [error, setError] = useState<string | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  useDocumentTitle(file?.name);

  useEffect(() => {
    let cancelled = false;
    let created: EditorStore | null = null;
    setError(null);
    Promise.all([api.files.get(fileId), loadEngine()])
      .then(async ([info]) => {
        if (cancelled) return;
        setFile(info);
        const engine = await createEditor(canvas, {
          // Send pending edits before reloading onto WebGL2.
          onGpuFailure: () => {
            created?.close();
            window.location.reload();
          },
        });
        if (cancelled) {
          engine.free();
          return;
        }
        created = new EditorStore(engine, fileId, { workspaceId: info.workspaceId, userId });
        created.onFatal = (m) => setFatal(m);
        created.connect(api.files.socketUrl(fileId));
        // Handy for debugging and profiling from the console.
        (window as unknown as { __flexma?: EditorStore }).__flexma = created;
        setStore(created);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(errorMessage(err, 'Could not open this file'));
      });
    return () => {
      cancelled = true;
      created?.close();
      created?.engine.free();
    };
  }, [fileId, canvas, userId]);

  if (error) return <FullScreenMessage title="Can't open file" message={error} homeLink />;
  if (fatal) return <FullScreenMessage title="File unavailable" message={fatal} homeLink />;
  if (!file || !store) return <FullScreenLoader label="Opening file" />;
  return <Editor store={store} canvas={canvas} header={<TopBar store={store} file={file} onRenamed={setFile} />} />;
}

/** The editor shell (panels, canvas, toolbar). Also used by the public share viewer. */
export function Editor({ store, canvas, header, footer }: { store: EditorStore; canvas: HTMLCanvasElement; header: ReactNode; footer?: ReactNode }) {
  const toast = useToast();
  const menu = useMenu();
  const ui = useUi(store);
  const selection = useSelection(store);
  const commentMode = useCommentMode(store);
  const [panels, setPanels] = useState(true);
  const e = store.engine;

  useEffect(() => {
    store.onReject = (m) => toast.error(`Change rejected: ${m}`, { id: 'reject' });
    store.onReload = () => toast.info('This file was restored to an earlier version.', { id: 'reload' });
  }, [store, toast]);

  // Cmd+\ toggles the UI panels (focus mode).
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if ((ev.metaKey || ev.ctrlKey) && ev.key === '\\') {
        ev.preventDefault();
        setPanels((p) => !p);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const onDropFiles = useCallback(
    async (files: File[], x: number, y: number) => {
      if (!store.engine.can_edit()) return;
      for (const [i, f] of files.entries()) {
        const id = toast.info(`Uploading ${f.name}…`, { duration: 0 });
        try {
          const a = await api.files.uploadAsset(store.fileId, f);
          store.engine.insert_image(a.id, a.width || 400, a.height || 300, x + i * 24, y + i * 24, x >= 0);
          store.poll();
          toast.dismiss(id);
        } catch (err) {
          toast.dismiss(id);
          toast.error(err);
        }
      }
    },
    [store, toast],
  );

  const act = (f: () => void) => () => {
    f();
    store.poll();
  };

  const onContextMenu = (ev: ReactMouseEvent<HTMLDivElement>) => {
    ev.preventDefault();
    const r = ev.currentTarget.getBoundingClientRect();
    const x = ev.clientX - r.left;
    const y = ev.clientY - r.top;
    const hit = e.hit_test(x, y, false);
    const selected = new Set<string>(JSON.parse(e.selection_ids()) as string[]);
    if (hit && !selected.has(hit)) e.set_selection(JSON.stringify([hit]));
    if (!hit && selected.size === 0) e.set_selection('[]');
    store.poll();
    const has = (JSON.parse(e.selection_ids()) as string[]).length > 0;
    const edit = e.can_edit();
    menu.open({ x: ev.clientX, y: ev.clientY }, [
      {
        label: 'Copy',
        hint: '⌘C',
        disabled: !has,
        onSelect: () => {
          const data = e.copy();
          if (data) void navigator.clipboard?.writeText(CLIP_PREFIX + data);
        },
      },
      {
        label: 'Paste',
        hint: '⌘V',
        disabled: !edit,
        onSelect: async () => {
          const text = await navigator.clipboard?.readText().catch(() => '');
          if (text?.startsWith(CLIP_PREFIX)) {
            e.paste(text.slice(CLIP_PREFIX.length));
            store.poll();
          }
        },
      },
      { label: 'Duplicate', hint: '⌘D', disabled: !has || !edit, onSelect: act(() => e.duplicate()) },
      { label: 'Delete', hint: '⌫', disabled: !has || !edit, danger: true, onSelect: act(() => e.delete_selection()) },
      { type: 'separator' },
      { label: 'Bring to front', hint: '⌥⌘]', disabled: !has || !edit, onSelect: act(() => e.reorder('front')) },
      { label: 'Bring forward', hint: '⌘]', disabled: !has || !edit, onSelect: act(() => e.reorder('forward')) },
      { label: 'Send backward', hint: '⌘[', disabled: !has || !edit, onSelect: act(() => e.reorder('backward')) },
      { label: 'Send to back', hint: '⌥⌘[', disabled: !has || !edit, onSelect: act(() => e.reorder('back')) },
      { type: 'separator' },
      { label: 'Group selection', hint: '⌘G', disabled: !has || !edit, onSelect: act(() => e.group()) },
      { label: 'Frame selection', hint: '⌥⌘G', disabled: !has || !edit, onSelect: act(() => e.frame_selection()) },
      { label: 'Ungroup', hint: '⇧⌘G', disabled: !has || !edit, onSelect: act(() => e.ungroup()) },
      { label: 'Add auto layout', hint: '⇧A', disabled: !has || !edit, onSelect: act(() => e.add_auto_layout()) },
      { type: 'separator' },
      { label: 'Create component', hint: '⌥⌘K', disabled: !has || !edit, onSelect: act(() => e.create_component()) },
      { label: 'Detach instance', hint: '⌥⌘B', disabled: !has || !edit, onSelect: act(() => e.detach_instances()) },
      {
        label: 'Go to main component',
        disabled: (JSON.parse(e.selection_ids()) as string[]).length !== 1,
        onSelect: act(() => {
          const [id] = JSON.parse(e.selection_ids()) as string[];
          if (id) e.go_to_main_component(id.split('~')[0]);
        }),
      },
      { type: 'separator' },
      { label: 'Show/Hide', hint: '⇧⌘H', disabled: !has || !edit, onSelect: act(() => e.toggle_visible(e.selection_ids())) },
      { label: 'Lock/Unlock', hint: '⇧⌘L', disabled: !has || !edit, onSelect: act(() => e.toggle_locked(e.selection_ids())) },
      { type: 'separator' },
      { label: 'Zoom to selection', hint: '⇧2', disabled: !has, onSelect: act(() => e.zoom_to_selection()) },
      { label: 'Zoom to fit', hint: '⇧1', onSelect: act(() => e.zoom_to_fit()) },
    ]);
  };

  return (
    <div className={`editor${panels ? '' : ' no-panels'}`}>
      {header}
      <div className="editor-body">
        {panels && (
          <aside className="editor-left">
            <LayersPanel store={store} />
          </aside>
        )}
        <main className="editor-center">
          <Canvas store={store} canvas={canvas} onContextMenu={onContextMenu} onDropFiles={onDropFiles} />
          <Toolbar store={store} />
          {footer}
        </main>
        {panels && (
          <aside className="editor-right" aria-label="Design properties">
            <div className="panel-tabs">
              <button type="button" className={`panel-tab${commentMode ? '' : ' active'}`} onClick={() => store.setCommentMode(false)}>
                {store.shareToken ? 'Inspect' : 'Design'}
              </button>
              {!store.shareToken && (
                <button type="button" className={`panel-tab${commentMode ? ' active' : ''}`} onClick={() => store.setCommentMode(true)}>
                  Comments
                </button>
              )}
              <span className="panel-meta">{commentMode ? '' : selection.length ? `${selection.length} selected` : ui.canEdit || store.shareToken ? '' : 'View only'}</span>
            </div>
            {commentMode ? <CommentsPanel store={store} /> : <PropertiesPanel store={store} />}
          </aside>
        )}
      </div>
      {menu.element}
    </div>
  );
}
