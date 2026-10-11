// Which computer the person is setting an agent up on, so setup snippets use
// the right shell: Terminal (zsh/bash) on macOS and Linux, PowerShell on Windows.
import { useState } from 'react';
import type { AgentSetup } from './api';
import { readStorage, writeStorage } from './storage';

export type OS = 'mac' | 'windows' | 'linux';

export const OS_LABELS: Record<OS, string> = { mac: 'macOS', windows: 'Windows', linux: 'Linux' };

const OS_KEY = 'flexma:agentOs';

export function detectOS(): OS {
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  const p = `${nav.userAgentData?.platform ?? ''} ${nav.platform ?? ''} ${nav.userAgent ?? ''}`.toLowerCase();
  if (p.includes('win')) return 'windows';
  if (p.includes('mac') || p.includes('iphone') || p.includes('ipad')) return 'mac';
  return 'linux';
}

/** The OS to show commands for: last picked, else the one we're running on. */
export function useAgentOS(): [OS, (os: OS) => void] {
  const [os, setOs] = useState<OS>(() => {
    const saved = readStorage(OS_KEY);
    return saved === 'mac' || saved === 'windows' || saved === 'linux' ? saved : detectOS();
  });
  return [
    os,
    (next: OS) => {
      setOs(next);
      writeStorage(OS_KEY, next);
    },
  ];
}

export interface OsSnippets {
  /** Where to paste the commands. */
  shell: string;
  claudeCommand: string;
  codexCommand: string;
  skillInstallCommand: string;
  /** Linux machines don't always have curl. */
  skillInstallFallback: string | null;
  claudeTokenCommand: string;
  codexTokenCommand: string;
  codexConfigPath: string;
  skillDirs: [string, string];
  /** How to make FLEXMA_TOKEN available to agents. */
  setTokenCommand: string;
}

export function snippetsFor(s: AgentSetup, os: OS): OsSnippets {
  if (os === 'windows' && s.windows) {
    return {
      shell: 'PowerShell',
      ...s.windows,
      skillInstallFallback: null,
      codexConfigPath: '%USERPROFILE%\\.codex\\config.toml',
      skillDirs: ['%USERPROFILE%\\.claude\\skills', '%USERPROFILE%\\.codex\\skills'],
      setTokenCommand: 'setx FLEXMA_TOKEN "$FLEXMA_TOKEN"',
    };
  }
  return {
    shell: 'Terminal',
    claudeCommand: s.claudeCommand,
    codexCommand: s.codexCommand,
    skillInstallCommand: s.skillInstallCommand,
    skillInstallFallback: os === 'linux' ? s.skillInstallCommand.replace('curl -fsSL', 'wget -qO-') : null,
    claudeTokenCommand: s.claudeTokenCommand,
    codexTokenCommand: s.codexTokenCommand,
    codexConfigPath: '~/.codex/config.toml',
    skillDirs: ['~/.claude/skills', '~/.codex/skills'],
    setTokenCommand: `echo 'export FLEXMA_TOKEN="$FLEXMA_TOKEN"' >> ${os === 'mac' ? '~/.zshrc' : '~/.bashrc'}`,
  };
}
