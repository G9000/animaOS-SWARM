import { useEffect, useMemo, useState } from 'react';
import type { Skill } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import {
  SLASH_COMMANDS,
  skillSlashCommands,
  type SlashCommand,
} from '../lib/slash-commands';

/** The composer's commands: the built-ins, then `/<slug>` for each enabled,
 *  active skill (spec §15.3), read again on `skill.updated`, a snapshot, or
 *  a resync. A failed read leaves the built-ins. */
export function useSkillCommands({
  version,
  epoch,
  enabled,
}: {
  version: number;
  epoch: number;
  enabled: boolean;
}): readonly SlashCommand[] {
  const [skills, setSkills] = useState<readonly Skill[]>([]);
  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    daemon.listSkills({ signal: controller.signal }).then(
      (next) => {
        if (controller.signal.aborted) return;
        // An empty list read again changes nothing: no re-render (I4).
        setSkills((previous) =>
          previous.length === 0 && next.length === 0 ? previous : next,
        );
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [enabled, version, epoch]);
  return useMemo(
    () =>
      skills.length === 0
        ? SLASH_COMMANDS
        : [...SLASH_COMMANDS, ...skillSlashCommands(skills)],
    [skills],
  );
}
