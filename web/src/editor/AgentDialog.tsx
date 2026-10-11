// "Connect AI" dialog: how to point Claude Code / Codex at this server via MCP,
// with commands for the person's OS (Terminal on macOS/Linux, PowerShell on Windows).
import { Link } from 'react-router-dom';
import { Modal } from '../components/Modal';
import { CopyField } from '../components/CopyField';
import { OsSwitch } from '../components/OsSwitch';
import { api, type AgentSetup } from '../lib/api';
import { snippetsFor, useAgentOS } from '../lib/platform';
import { useEffect, useState } from 'react';

export function AgentDialog({ open, onClose, fileId, fileName }: { open: boolean; onClose: () => void; fileId: string; fileName: string }) {
  const [setup, setSetup] = useState<AgentSetup | null>(null);
  const [os, setOs] = useAgentOS();
  useEffect(() => {
    if (open && !setup) api.agentSetup().then(setSetup).catch(() => undefined);
  }, [open, setup]);
  const prompt = `In my Flexma file "${fileName}" (file id ${fileId}), `;
  const s = setup && snippetsFor(setup, os);
  return (
    <Modal open={open} onClose={onClose} title="Design with AI agents" description="Claude Code, Codex and any MCP client can read and edit this file live. Their changes appear here instantly." size="md">
      {!s ? (
        <div className="skeleton skeleton-block" style={{ height: 120 }} />
      ) : (
        <div className="stack">
          <div className="os-row">
            <span className="text-muted text-sm">Commands for</span>
            <OsSwitch value={os} onChange={setOs} />
          </div>
          <div className="field">
            <div className="field-label">1. Connect Claude Code: run in {s.shell} (opens your browser to sign in)</div>
            <CopyField value={s.claudeCommand} multiline toastMessage="Command copied" />
          </div>
          <div className="field">
            <div className="field-label">…or Codex</div>
            <CopyField value={s.codexCommand} multiline toastMessage="Command copied" />
          </div>
          <div className="field">
            <div className="field-label">2. Teach your agents Flexma (installs the skill for Claude Code and Codex)</div>
            <CopyField value={s.skillInstallCommand} multiline toastMessage="Command copied" />
            {s.skillInstallFallback && (
              <p className="text-muted text-sm">
                No curl? Use <code>{s.skillInstallFallback}</code>
              </p>
            )}
          </div>
          <div className="field">
            <div className="field-label">3. Ask away, e.g.</div>
            <CopyField value={`${prompt}design a pricing section with three plans using auto layout.`} multiline toastMessage="Prompt copied" />
          </div>
          <p className="text-muted text-sm">
            Need a token for headless agents or CI? Create one on the <Link to="/agents">AI Agents</Link> page.
          </p>
        </div>
      )}
    </Modal>
  );
}
