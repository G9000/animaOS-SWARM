import type { DaemonClient } from './client.js';

/** Exact and automatic sizing are mutually exclusive; counts include the manager. */
export type AgencyGenerateRequest = {
  name: string;
  description: string;
  provider: string;
  model: string;
  modelPool?: string[];
} & (
  | { teamSize: number; maxTeamSize?: never }
  | { teamSize?: never; maxTeamSize: number }
);

/** Generated tools are suggestions, not granted permissions. */
export interface AgentDefinitionResponse {
  name: string;
  position: string | null;
  role: string;
  bio: string | null;
  lore: string | null;
  adjectives: string[] | null;
  topics: string[] | null;
  knowledge: string[] | null;
  style: string | null;
  system: string | null;
  model: string | null;
  tools: string[] | null;
  collaboratesWith: string[] | null;
}

export interface AgencyGenerateResponse {
  name: string;
  description: string;
  provider: string;
  model: string;
  teamSize: number;
  mission: string | null;
  values: string[] | null;
  agents: AgentDefinitionResponse[];
}

export class AgenciesClient {
  constructor(private readonly client: DaemonClient) {}

  generate(input: AgencyGenerateRequest): Promise<AgencyGenerateResponse> {
    return this.client.requestJson('/api/agencies/generate', {
      method: 'POST',
      body: input,
    });
  }
}
