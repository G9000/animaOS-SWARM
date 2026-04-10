import type { AgentEntry, AgentProfile } from './types.js';

export function buildDisplayedAgents(
  profiles: AgentProfile[],
  liveAgents: AgentEntry[]
): AgentEntry[] {
  if (profiles.length === 0) {
    return liveAgents;
  }

  const liveByName = new Map(liveAgents.map((agent) => [agent.name, agent]));
  const configuredAgents = profiles.map((profile) => {
    const liveAgent = liveByName.get(profile.name);

    if (liveAgent) {
      liveByName.delete(profile.name);
      return liveAgent;
    }

    return {
      id: `profile:${profile.name}`,
      name: profile.name,
      status: 'idle',
      tokens: 0,
    } satisfies AgentEntry;
  });

  return [...configuredAgents, ...liveByName.values()];
}
