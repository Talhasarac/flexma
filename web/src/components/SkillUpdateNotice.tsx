// "Flexma skills were updated": which ones, and the command that updates
// every installed copy (agents keep the copy they installed). Without a
// command, it points at the one shown below it.
import { Fragment } from 'react';
import { Sparkles } from 'lucide-react';
import type { SkillInfo } from '../lib/api';
import { CopyField } from './CopyField';
import { TimeAgo } from './TimeAgo';

export function SkillUpdateNotice({ skills, command }: { skills: SkillInfo[]; command?: string }) {
  if (skills.length === 0) return null;
  const latest = skills.reduce((a, b) => (Date.parse(a.updatedAt) > Date.parse(b.updatedAt) ? a : b));
  return (
    <div className="callout callout-accent" role="status">
      <div className="callout-title">
        <Sparkles size={14} aria-hidden="true" /> Skill update
      </div>
      <p className="text-sm">
        {skills.map((s, i) => (
          <Fragment key={s.name}>
            {i > 0 && (i === skills.length - 1 ? ' and ' : ', ')}
            <code>{s.name}</code>
          </Fragment>
        ))}{' '}
        {skills.length === 1 ? 'was' : 'were'} updated <TimeAgo date={latest.updatedAt} />. Your agents keep the copy they
        installed. {command ? 'Run this' : 'Run the command below again'} to update every Flexma skill you have{command ? ':' : '.'}
      </p>
      {command && <CopyField value={command} multiline toastMessage="Command copied" />}
      <p className="text-muted text-sm">Agents that connect to Flexma are told too, and offer to update.</p>
    </div>
  );
}
