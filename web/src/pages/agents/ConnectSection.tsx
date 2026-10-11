import { useState, type ReactElement } from 'react';
import { Info } from 'lucide-react';
import type { AgentSetup } from '../../lib/api';
import { snippetsFor, type OS } from '../../lib/platform';
import type { AsyncState } from '../../lib/useAsync';
import { CopyField } from '../../components/CopyField';
import { ErrorState } from '../../components/EmptyState';
import { Tabs } from '../../components/Tabs';

type AgentTab = 'claude' | 'codex' | 'other';

// `$FLEXMA_TOKEN` (sh) or `$env:FLEXMA_TOKEN` (PowerShell).
const PLACEHOLDER = /\$(?:env:)?\{?FLEXMA_TOKEN\}?/g;

/** Replace `$FLEXMA_TOKEN` placeholders with a real token when we have one. */
export function fillToken(snippet: string, token: string | undefined): string {
  return token ? snippet.replace(PLACEHOLDER, token) : snippet;
}

interface ConnectProps {
  /** `GET /api/agent-setup`, loaded once by the page. */
  setup: AsyncState<AgentSetup>;
  /** The computer being set up; picks Terminal or PowerShell commands. */
  os: OS;
  token?: string;
}

function SetupLoading({ setup }: { setup: AsyncState<AgentSetup> }) {
  if (setup.error)
    return <ErrorState error={setup.error} onRetry={() => void setup.reload()} title="Couldn't load setup instructions" />;
  return (
    <div className="list-placeholder">
      <span className="skeleton skeleton-block" />
    </div>
  );
}

function AgentTabs({ children }: { children: (tab: AgentTab) => ReactElement }) {
  const [tab, setTab] = useState<AgentTab>('claude');
  return (
    <Tabs
      label="Agent"
      value={tab}
      onChange={setTab}
      tabs={[
        { id: 'claude', label: 'Claude Code' },
        { id: 'codex', label: 'Codex' },
        { id: 'other', label: 'Other MCP clients' },
      ]}
    >
      {children(tab)}
    </Tabs>
  );
}

/** Step 1: connect with browser sign-in (OAuth), like hosted MCP servers. */
export function ConnectSection({ setup, os }: ConnectProps) {
  if (!setup.data) return <SetupLoading setup={setup} />;
  const s = setup.data;
  const o = snippetsFor(s, os);
  return (
    <AgentTabs>
      {(tab) =>
        tab === 'claude' ? (
          <div className="stack">
            <p className="text-muted">Run this once in {o.shell}:</p>
            <CopyField value={o.claudeCommand} multiline toastMessage="Command copied" />
            <p className="text-muted text-sm">
              Then type <code>/mcp</code> in Claude Code, pick <b>flexma</b> and choose <b>Authenticate</b>. Your browser opens
              this server's sign-in page. Approve access and you're connected.
            </p>
          </div>
        ) : tab === 'codex' ? (
          <div className="stack">
            <p className="text-muted">Add Flexma and sign in through your browser (in {o.shell}):</p>
            <CopyField value={o.codexCommand} multiline toastMessage="Command copied" />
            <p className="text-muted">
              Or add it to <code>{o.codexConfigPath}</code> and run <code>codex mcp login flexma</code>:
            </p>
            <CopyField value={s.codexConfig} multiline toastMessage="Config copied" />
          </div>
        ) : (
          <div className="stack">
            <p className="text-muted">
              Flexma serves MCP over Streamable HTTP with OAuth 2.1 (dynamic client registration and PKCE), so any MCP client that
              supports remote servers can connect to this URL:
            </p>
            <CopyField value={s.mcpUrl} label="MCP endpoint" toastMessage="URL copied" />
          </div>
        )
      }
    </AgentTabs>
  );
}

/** Optional: bearer-token setup for headless agents and CI. */
export function TokenConnectSection({ setup, os, token }: ConnectProps) {
  if (!setup.data) return <SetupLoading setup={setup} />;
  const s = setup.data;
  const o = snippetsFor(s, os);
  return (
    <div className="stack">
      {!token && <TokenHint os={os} />}
      <AgentTabs>
        {(tab) =>
          tab === 'claude' ? (
            <CopyField value={fillToken(o.claudeTokenCommand, token)} multiline toastMessage="Command copied" />
          ) : tab === 'codex' ? (
            <div className="stack">
              <p className="text-muted text-sm">Codex reads the token from the FLEXMA_TOKEN environment variable. Save it once:</p>
              <CopyField value={fillToken(o.setTokenCommand, token)} multiline toastMessage="Command copied" />
              <p className="text-muted text-sm">
                {os === 'windows' ? 'Then open a new PowerShell window and run:' : 'Then open a new terminal and run:'}
              </p>
              <CopyField value={fillToken(o.codexTokenCommand, token)} multiline toastMessage="Command copied" />
              <p className="text-muted text-sm">
                Or put this in <code>{o.codexConfigPath}</code>:
              </p>
              <CopyField value={fillToken(s.codexTokenConfig, token)} multiline toastMessage="Config copied" />
            </div>
          ) : (
            <div className="stack">
              <CopyField value={s.mcpUrl} label="MCP endpoint" toastMessage="URL copied" />
              <CopyField value={`Authorization: Bearer ${token ?? '$FLEXMA_TOKEN'}`} label="Header" toastMessage="Header copied" />
            </div>
          )
        }
      </AgentTabs>
    </div>
  );
}

/** The Flexma skill for Claude Code and Codex. */
export function SkillSection({ setup, os }: ConnectProps) {
  if (!setup.data) return <SetupLoading setup={setup} />;
  const o = snippetsFor(setup.data, os);
  return (
    <div className="stack">
      <p className="text-muted">Run in {o.shell}:</p>
      <CopyField value={o.skillInstallCommand} multiline toastMessage="Command copied" />
      {o.skillInstallFallback && (
        <p className="text-muted text-sm">
          No curl? Use <code>{o.skillInstallFallback}</code>
        </p>
      )}
      <p className="text-muted text-sm">
        Installs the <code>flexma</code> skill into <code>{o.skillDirs[0]}</code> and <code>{o.skillDirs[1]}</code>.
      </p>
    </div>
  );
}

function TokenHint({ os }: { os: OS }) {
  return (
    <p className="hint-row">
      <Info size={14} aria-hidden="true" />
      <span>
        Commands use <code>{os === 'windows' ? '$env:FLEXMA_TOKEN' : '$FLEXMA_TOKEN'}</code> as a placeholder. Create a
        token above and it is filled in for you.
      </span>
    </p>
  );
}
