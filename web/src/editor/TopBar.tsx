// Editor top bar: navigation, file name, presence, sync status, zoom and menus.
import { useState } from 'react';
import { Link } from 'react-router-dom';
import { ChevronDown, CloudOff, Eye, Loader2, Menu as MenuIcon, Redo2, Share2, Sparkles, Undo2 } from 'lucide-react';
import { Avatar } from '../components/Avatar';
import { useMenu } from '../components/Menu';
import { useToast } from '../components/Toast';
import { useDialogs } from '../components/Dialogs';
import { api, type FileInfo } from '../lib/api';
import { usePeers, useSaving, useStatus, useUi, type EditorStore } from './store';
import { AgentDialog } from './AgentDialog';
import { ShareDialog } from '../components/ShareDialog';
import { VersionsDialog } from './VersionsDialog';
import { MissingFontsButton } from './MissingFonts';
import { exportPngs, safeName } from './download';
import { useAuth } from '../lib/auth';

interface Props {
  store: EditorStore;
  file: FileInfo;
  onRenamed: (f: FileInfo) => void;
}

export function TopBar({ store, file, onRenamed }: Props) {
  const ui = useUi(store);
  const status = useStatus(store);
  const saving = useSaving(store);
  const menu = useMenu();
  const toast = useToast();
  const dialogs = useDialogs();
  const { user } = useAuth();
  const [agentsOpen, setAgentsOpen] = useState(false);
  const [versionsOpen, setVersionsOpen] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const e = store.engine;
  const act = (f: () => void) => () => {
    f();
    store.poll();
  };

  // A painting file shows the brush and eraser; any file can switch.
  const togglePainting = async () => {
    const kind = store.paintTools ? 'design' : 'painting';
    try {
      onRenamed(await api.files.update(file.id, { kind }));
      store.setPaintTools(kind === 'painting');
    } catch (err) {
      toast.error(err);
    }
  };

  const rename = async () => {
    if (!ui.canEdit) return;
    const name = await dialogs.prompt({ title: 'Rename file', defaultValue: file.name, maxLength: 200 });
    if (!name || name === file.name) return;
    try {
      onRenamed(await api.files.update(file.id, { name }));
    } catch (err) {
      toast.error(err);
    }
  };

  return (
    <header className="editor-topbar">
      <div className="tb-left">
        <button
          type="button"
          className="icon-btn tb-menu"
          aria-label="Main menu"
          onClick={(ev) =>
            menu.open(ev.currentTarget, [
              { label: 'Back to files', onSelect: () => (window.location.href = `/w/${file.workspaceId}/p/${file.projectId}`) },
              { type: 'separator' },
              { label: 'Undo', hint: '⌘Z', disabled: !ui.canUndo, onSelect: act(() => e.undo()) },
              { label: 'Redo', hint: '⇧⌘Z', disabled: !ui.canRedo, onSelect: act(() => e.redo()) },
              { type: 'separator' },
              { label: 'Export page as PNG', onSelect: () => void exportPngs(toast, [{ url: store.exportPngUrl({ page: ui.page, scale: 2 }), name: `${safeName(file.name)}@2x.png` }]) },
              { label: 'Export page as SVG', onSelect: () => window.open(store.exportSvgUrl({ page: ui.page }), '_blank') },
              { type: 'separator' },
              { label: 'Rename file…', disabled: !ui.canEdit, onSelect: () => void rename() },
              { label: 'Painting tools', checked: store.paintTools, disabled: !ui.canEdit, onSelect: () => void togglePainting() },
              { label: 'Share…', onSelect: () => setShareOpen(true) },
              { label: 'Version history…', onSelect: () => setVersionsOpen(true) },
              { label: 'Connect AI agents…', onSelect: () => setAgentsOpen(true) },
            ])
          }
        >
          <MenuIcon size={16} />
        </button>
        <div className="tb-title">
          <Link to={`/w/${file.workspaceId}/p/${file.projectId}`} className="tb-project">
            {file.projectName}
          </Link>
          <span className="tb-slash">/</span>
          <button type="button" className="tb-filename" onClick={() => void rename()} title={ui.canEdit ? 'Rename' : file.name}>
            {file.name}
          </button>
          <span className="tb-status" title={status === 'offline' ? 'Offline — reconnecting. Your changes are kept and will sync.' : saving ? 'Saving…' : 'All changes saved'}>
            {status === 'offline' ? <CloudOff size={13} /> : saving ? <Loader2 size={13} className="spin" /> : null}
          </span>
        </div>
      </div>

      <div className="tb-right">
        <button type="button" className="icon-btn" title="Undo (⌘Z)" aria-label="Undo" disabled={!ui.canUndo} onClick={act(() => e.undo())}>
          <Undo2 size={15} />
        </button>
        <button type="button" className="icon-btn" title="Redo (⇧⌘Z)" aria-label="Redo" disabled={!ui.canRedo} onClick={act(() => e.redo())}>
          <Redo2 size={15} />
        </button>
        <MissingFontsButton store={store} />
        <Presence store={store} self={user ? { name: user.name, color: user.color } : null} />
        <button type="button" className="btn btn-sm tb-ai" onClick={() => setAgentsOpen(true)} title="Connect Claude Code, Codex or OpenCode">
          <Sparkles size={14} /> AI
        </button>
        <button type="button" className="btn btn-sm btn-primary" onClick={() => setShareOpen(true)}>
          <Share2 size={14} /> Share
        </button>
        <ZoomButton store={store} />
      </div>
      {menu.element}
      <AgentDialog open={agentsOpen} onClose={() => setAgentsOpen(false)} fileId={file.id} fileName={file.name} />
      <VersionsDialog open={versionsOpen} onClose={() => setVersionsOpen(false)} fileId={file.id} canEdit={ui.canEdit} />
      <ShareDialog open={shareOpen} onClose={() => setShareOpen(false)} file={file} />
    </header>
  );
}

/** Avatars of everyone else in the file (agents as chips, link guests grouped). */
export function Presence({ store, self }: { store: EditorStore; self: { name: string; color: string } | null }) {
  const peers = usePeers(store);
  const ui = useUi(store);
  const e = store.engine;
  const agents = peers.filter((p) => p.isAgent);
  const guests = peers.filter((p) => p.isGuest && !p.isAgent);
  const humans = peers.filter((p) => !p.isAgent && !p.isGuest);
  return (
    <div className="presence">
      {agents.map((p) => (
        <span key={p.clientId} className="agent-chip" style={{ background: p.color }} title={`${p.name} is editing`}>
          <Sparkles size={11} /> {p.name.split(' · ')[0]}
        </span>
      ))}
      {humans.slice(0, 5).map((p) => (
        <button
          type="button"
          key={p.clientId}
          className="presence-avatar"
          title={`${p.name} — click to jump to their view`}
          onClick={() => {
            if (p.page && p.page !== ui.page) e.set_page(p.page);
            if (p.selection[0]) e.focus_node(p.selection[0]);
            store.poll();
          }}
        >
          <Avatar name={p.name} color={p.color} size={26} />
        </button>
      ))}
      {humans.length > 5 && <span className="presence-more">+{humans.length - 5}</span>}
      {guests.length > 0 && (
        <span className="presence-guests" title={`${guests.length} ${guests.length === 1 ? 'person is' : 'people are'} viewing through the public link`}>
          <Eye size={12} /> {guests.length}
        </span>
      )}
      {self && <Avatar name={self.name} color={self.color} size={26} title={`${self.name} (you)`} />}
    </div>
  );
}

export function ZoomButton({ store }: { store: EditorStore }) {
  const ui = useUi(store);
  const zoomMenu = useMenu();
  const e = store.engine;
  const act = (f: () => void) => () => {
    f();
    store.poll();
  };
  return (
    <>
      <button
        type="button"
        className="zoom-btn"
        onClick={(ev) =>
          zoomMenu.open(
            ev.currentTarget,
            [
              { label: 'Zoom in', hint: '+', onSelect: act(() => e.zoom_in()) },
              { label: 'Zoom out', hint: '−', onSelect: act(() => e.zoom_out()) },
              { label: 'Zoom to fit', hint: '⇧1', onSelect: act(() => e.zoom_to_fit()) },
              { label: 'Zoom to selection', hint: '⇧2', onSelect: act(() => e.zoom_to_selection()) },
              { label: 'Zoom to 50%', onSelect: act(() => e.set_zoom(0.5)) },
              { label: 'Zoom to 100%', hint: '⇧0', onSelect: act(() => e.set_zoom(1)) },
              { label: 'Zoom to 200%', onSelect: act(() => e.set_zoom(2)) },
            ],
            { align: 'end' },
          )
        }
      >
        {Math.round(ui.zoom * 100)}% <ChevronDown size={12} />
      </button>
      {zoomMenu.element}
    </>
  );
}
