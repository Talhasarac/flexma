// "Connect AI" dialog: how to point Claude Code / Codex at this server via MCP.
import { Link } from 'react-router-dom';
import { Modal } from '../components/Modal';
import { CopyField } from '../components/CopyField';
import { api, type AgentSetup } from '../lib/api';
import { useEffect, useState } from 'react';

export function AgentDialog({ open, onClose, fileId, fileName }: { open: boolean; onClose: () => void; fileId: string; fileName: string }) {
  const [setup, setSetup] = useState<AgentSetup | null>(null);
  useEffect(() => {
    if (open && !setup) api.agentSetup().then(setSetup).catch(() => undefined);
  }, [open, setup]);
  const prompt = `In my Flexma file "${fileName}" (file id ${fileId}), `;
  return (
    <Modal open={open} onClose={onClose} title="Design with AI agents" description="Claude Code, Codex and any MCP client can read and edit this file live. Their changes appear here instantly." size="md">
      {!setup ? (
        <div className="skeleton skeleton-block" style={{ height: 120 }} />
      ) : (
        <div className="stack">
          <div className="field">
            <div className="field-label">1. Connect Claude Code (opens your browser to sign in)</div>
            <CopyField value={setup.claudeCommand} multiline toastMessage="Command copied" />
          </div>
          <div className="field">
            <div className="field-label">…or Codex</div>
            <CopyField value={setup.codexCommand} multiline toastMessage="Command copied" />
          </div>
          <div className="field">
            <div className="field-label">2. Teach your agents Flexma (installs the skill for Claude Code and Codex)</div>
            <CopyField value={setup.skillInstallCommand} toastMessage="Command copied" />
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
