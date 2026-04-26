export interface AgentDefinition {
  name: string;
  position?: string;
  bio: string;
  lore?: string;
  knowledge?: string[];
  topics?: string[];
  adjectives?: string[];
  style?: string;
  system: string;
  role?: 'orchestrator' | 'worker';
  model?: string;
  tools?: string[];
  /** Names of other agents this one frequently collaborates with — drives the org chart edges. */
  collaboratesWith?: string[];
}

export interface AgencyConfig {
  name: string;
  description: string;
  /** One-sentence north star the whole team shares. */
  mission?: string;
  /** 3-5 cultural principles every agent operates under. */
  values?: string[];
  model: string;
  provider: string;
  strategy: 'supervisor' | 'dynamic' | 'round-robin';
  maxParallelDelegations?: number;
  orchestrator: AgentDefinition;
  agents: AgentDefinition[];
}
