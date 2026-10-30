// Agent skill updates: the skills that changed since this person last looked
// (the dot on the editor's AI button), and a way to say they've seen them.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, type SkillInfo } from './api';

export function useSkillUpdates(): { updated: SkillInfo[]; markSeen: () => void } {
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  useEffect(() => {
    let live = true;
    api.skills
      .list()
      .then((r) => live && setSkills(r.skills))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);
  const updated = useMemo(() => skills.filter((s) => s.updated), [skills]);
  const markSeen = useCallback(() => {
    if (updated.length === 0) return;
    setSkills((all) => all.map((s) => ({ ...s, updated: false })));
    api.skills.seen().catch(() => undefined);
  }, [updated.length]);
  return { updated, markSeen };
}
