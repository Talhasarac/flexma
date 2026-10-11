import { useState, type ReactNode } from 'react';
import { Bot, Infinity as InfinityIcon, MousePointer2, ShieldCheck } from 'lucide-react';
import { api, type CreatedApiToken } from '../../lib/api';
import { useAsync } from '../../lib/useAsync';
import { useDocumentTitle } from '../../lib/useDocumentTitle';
import { useAgentOS } from '../../lib/platform';
import { OsSwitch } from '../../components/OsSwitch';
import { PageHeader } from '../../components/dashboard/PageHeader';
import { ConnectSection, SkillSection, TokenConnectSection } from './ConnectSection';
import { TokensSection } from './TokensSection';

export default function AgentsPage() {
  useDocumentTitle('AI Agents');
  const [fresh, setFresh] = useState<CreatedApiToken | null>(null);
  const setup = useAsync(() => api.agentSetup(), 'agent-setup');
  const [os, setOs] = useAgentOS();

  return (
    <div className="page page-narrow">
      <PageHeader title="AI Agents" subtitle="Design together with Claude Code, Codex and any other MCP-capable agent." />

      <section className="card agents-hero">
        <div className="agents-hero-icon" aria-hidden="true">
          <Bot size={22} />
        </div>
        <div className="stack">
          <h2 className="agents-hero-title">Bring your coding agents into the canvas</h2>
          <p className="text-muted">
            Flexma speaks the Model Context Protocol (MCP). Connect Claude Code or Codex like any hosted MCP server: add the
            URL and sign in with your browser. They can then open, create and edit your design files, and see what they make.
          </p>
          <ul className="feature-list">
            <Feature icon={<InfinityIcon size={16} />} title="Unlimited connections">
              No seats and no caps. Run as many agents and parallel sessions as your server can handle.
            </Feature>
            <Feature icon={<MousePointer2 size={16} />} title="Live collaborators">
              Agents join files like teammates, with their own cursor, so you can watch them work in real time.
            </Feature>
            <Feature icon={<ShieldCheck size={16} />} title="Scoped to you">
              Agents act as you and see only the workspaces you can see. Revoke access any time below.
            </Feature>
          </ul>
        </div>
      </section>

      <div className="os-row agents-os">
        <span className="text-muted">Setting up on</span>
        <OsSwitch value={os} onChange={setOs} />
      </div>

      <div className="steps">
        <Step n={1} title="Connect your agent" description="Sign in through your browser. No tokens to copy, just like Figma's MCP.">
          <ConnectSection setup={setup} os={os} />
        </Step>
        <Step
          n={2}
          title="Install the Flexma skill (recommended)"
          description="Teaches your agent Flexma's tools and design conventions, so results look right the first time."
        >
          <SkillSection setup={setup} os={os} />
        </Step>
        <Step
          n={3}
          title="Connected agents & API tokens"
          description="Browser sign-ins show up here as '(OAuth)' tokens. For headless agents or CI, create a token and send it as a bearer header."
        >
          <TokensSection fresh={fresh} onCreated={setFresh} onDismissFresh={() => setFresh(null)} />
          {fresh && <TokenConnectSection setup={setup} os={os} token={fresh.token} />}
        </Step>
      </div>
    </div>
  );
}

function Feature({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <li className="feature">
      <span className="feature-icon" aria-hidden="true">
        {icon}
      </span>
      <span>
        <strong className="text-strong">{title}.</strong> <span className="text-muted">{children}</span>
      </span>
    </li>
  );
}

function Step({ n, title, description, children }: { n: number; title: string; description: string; children: ReactNode }) {
  return (
    <section className="step" aria-labelledby={`step-${n}`}>
      <div className="step-number" aria-hidden="true">
        {n}
      </div>
      <div className="step-body">
        <h2 className="step-title" id={`step-${n}`}>
          {title}
        </h2>
        <p className="step-description">{description}</p>
        {children}
      </div>
    </section>
  );
}
