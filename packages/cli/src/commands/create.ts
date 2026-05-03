import { Command } from 'commander';
import * as clack from '@clack/prompts';
import pc from 'picocolors';
import { writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import yaml from 'js-yaml';
import {
  generateAgentTeam,
  generateAgentSeeds,
  createAdapter,
} from '../agency/generator.js';
import type { AgencyConfig, AgentDefinition } from '../agency/types.js';
import {
  agentSlug,
  renderOrgChart,
  renderAgencyBrief,
  renderAgentProfile,
} from '../agency/diagram.js';
import {
  DETERMINISTIC_PROVIDER,
  PROVIDER_HELP_TEXT,
  REMOTE_PROVIDER_IDS,
  hasProviderApiKey,
  normalizeProvider,
  providerKeyEnvNames,
  providerRequiresApiKey,
} from '../provider-config.js';

export type CreateKind = 'agent' | 'agency';

export interface CreateOptions {
  kind?: string;
  provider?: string;
  model: string;
  description?: string;
  size?: string;
  apiKey?: string;
  models?: string;
  seed?: boolean;
  yes?: boolean;
  system?: string;
}

interface CreateTarget {
  kind?: CreateKind;
  nameArg?: string;
  needsKindPrompt: boolean;
  error?: string;
}

interface WrittenWorkspace {
  dirName: string;
  agentCount: number;
  seedAgentCount: number;
}

const TEAM_MIN = 2;
const TEAM_MAX = 10;
const DEFAULT_CREATE_MODEL = 'gpt-4o-mini';

function cancelAndExit(): never {
  clack.cancel('Cancelled.');
  process.exit(0);
}

function parseCreateKind(value: string | undefined): CreateKind | undefined {
  const normalized = value?.trim().toLowerCase();
  if (normalized === 'agent' || normalized === 'agency') return normalized;

  return undefined;
}

export function resolveCreateTarget(
  kindOrName: string | undefined,
  nameArg: string | undefined,
  kindOption: string | undefined
): CreateTarget {
  const optionKind = parseCreateKind(kindOption);
  if (kindOption && !optionKind) {
    return {
      needsKindPrompt: false,
      error: `Unknown create kind "${kindOption}". Use "agent" or "agency".`,
    };
  }

  if (optionKind) {
    return {
      kind: optionKind,
      nameArg: nameArg ?? (parseCreateKind(kindOrName) ? undefined : kindOrName),
      needsKindPrompt: false,
    };
  }

  const positionalKind = parseCreateKind(kindOrName);
  if (positionalKind) {
    return {
      kind: positionalKind,
      nameArg,
      needsKindPrompt: false,
    };
  }

  if (kindOrName && nameArg) {
    return {
      needsKindPrompt: false,
      error: `Unknown create kind "${kindOrName}". Use "agent" or "agency" before the name.`,
    };
  }

  if (kindOrName) {
    return {
      kind: 'agency',
      nameArg: kindOrName,
      needsKindPrompt: false,
    };
  }

  return { needsKindPrompt: true };
}

export function resolveCreateProvider(
  provider: string | undefined,
  apiKey?: string
): string {
  const explicitProvider = normalizeProvider(provider);
  if (explicitProvider) return explicitProvider;

  const configuredProvider = REMOTE_PROVIDER_IDS.find(
    (candidate) => providerRequiresApiKey(candidate) && hasProviderApiKey(candidate, apiKey)
  );

  return configuredProvider ?? DETERMINISTIC_PROVIDER;
}

export function getCreateProviderIssue(provider: string | undefined): string | undefined {
  const normalizedProvider = normalizeProvider(provider);
  if (
    !normalizedProvider ||
    normalizedProvider === DETERMINISTIC_PROVIDER ||
    REMOTE_PROVIDER_IDS.includes(normalizedProvider)
  ) {
    return undefined;
  }

  return `Unknown provider "${provider}". ${PROVIDER_HELP_TEXT}`;
}

export function getCreateGenerationCredentialIssue(
  provider: string,
  apiKey?: string
): string | undefined {
  if (!providerRequiresApiKey(provider) || hasProviderApiKey(provider, apiKey)) {
    return undefined;
  }

  const envNames = providerKeyEnvNames(provider);
  const envText = envNames.length > 0 ? envNames.join(', ') : 'the provider API key';
  return `Provider "${provider}" needs credentials before agency generation. Set ${envText}, pass --api-key, or use --provider ${DETERMINISTIC_PROVIDER} for a local scaffold.`;
}

function workspaceSlug(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, '')
    .replace(/\s+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-+|-+$/g, '');

  return slug || 'anima-workspace';
}

function splitModels(models: string | undefined): string[] | undefined {
  const modelPool = models
    ? models.split(',').map((model) => model.trim()).filter(Boolean)
    : [];

  return modelPool.length > 0 ? modelPool : undefined;
}

function toAgentDef(agent: AgentDefinition): AgentDefinition {
  return {
    name: agent.name,
    position: agent.position,
    bio: agent.bio,
    lore: agent.lore,
    adjectives: agent.adjectives,
    topics: agent.topics,
    knowledge: agent.knowledge,
    style: agent.style,
    system: agent.system,
    model: agent.model,
    tools: agent.tools,
    collaboratesWith: agent.collaboratesWith,
  };
}

function inferAgentTopics(description: string): string[] {
  const topics = description
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, ' ')
    .split(/\s+/)
    .filter((word) => word.length > 3)
    .filter(
      (word) => !['agent', 'build', 'create', 'that', 'this', 'with'].includes(word)
    );

  return [...new Set(topics)].slice(0, 5);
}

export function buildSingleAgentConfig(input: {
  name: string;
  description: string;
  provider: string;
  model: string;
  system?: string;
}): AgencyConfig {
  const description = input.description.trim();
  const system =
    input.system?.trim() ||
    `You are ${input.name}. ${description}. Be direct, preserve useful context, and challenge assumptions when a better path is visible.`;

  return {
    name: input.name,
    description,
    mission: description,
    values: [
      'Keep context useful',
      'Prefer concrete next steps',
      'Challenge weak assumptions',
    ],
    model: input.model,
    provider: input.provider,
    strategy: 'supervisor',
    orchestrator: {
      name: input.name,
      position: 'Agent',
      role: 'orchestrator',
      bio: description,
      lore: 'Created as a single-agent workspace with animaos create agent.',
      adjectives: ['focused', 'curious', 'direct'],
      topics: inferAgentTopics(description),
      knowledge: [`Primary mandate: ${description}`],
      style: 'Clear, practical, and concise. Calls out uncertainty instead of hiding it.',
      system,
      model: input.model,
      tools: [],
      collaboratesWith: [],
    },
    agents: [],
  };
}

function writeWorkspace(
  config: AgencyConfig,
  allAgentDefs: AgentDefinition[],
  seedsByName: Map<string, string>
): WrittenWorkspace {
  const dirName = workspaceSlug(config.name);
  const dirPath = join(process.cwd(), dirName);

  if (!existsSync(dirPath)) {
    mkdirSync(dirPath, { recursive: true });
  }

  writeFileSync(
    join(dirPath, 'anima.yaml'),
    yaml.dump(config, { lineWidth: 120, noRefs: true })
  );

  writeFileSync(join(dirPath, 'org-chart.mmd'), renderOrgChart(config));
  writeFileSync(join(dirPath, 'README.md'), renderAgencyBrief(config));

  const agentsRoot = join(dirPath, 'agents');
  mkdirSync(agentsRoot, { recursive: true });

  for (const agent of allAgentDefs) {
    const slug = agentSlug(agent.name);
    const agentDir = join(agentsRoot, slug);
    mkdirSync(join(agentDir, 'assets'), { recursive: true });
    mkdirSync(join(agentDir, 'memory'), { recursive: true });
    writeFileSync(join(agentDir, 'profile.md'), renderAgentProfile(agent, config.name));
    writeFileSync(join(agentDir, 'assets', '.gitkeep'), '');
    const seedJson = seedsByName.get(agent.name);
    if (seedJson) {
      writeFileSync(join(agentDir, 'memory', 'seed.json'), seedJson);
    } else {
      writeFileSync(join(agentDir, 'memory', '.gitkeep'), '');
    }
  }

  return {
    dirName,
    agentCount: allAgentDefs.length,
    seedAgentCount: seedsByName.size,
  };
}

function logWorkspaceResult(workspace: WrittenWorkspace, noun: 'Agent' | 'Agency') {
  clack.log.success(`${noun} saved to ${pc.green(workspace.dirName + '/')}`);
  const seedNote =
    workspace.seedAgentCount > 0
      ? `\n  ${pc.dim('•')} ${pc.green('agents/*/memory/seed.json')}  ${pc.dim(
          '- starter memories (' + workspace.seedAgentCount + ' agents)'
        )}`
      : '';

  clack.log.message(
    `  ${pc.dim('•')} ${pc.green('anima.yaml')}     ${pc.dim('- runtime config')}\n` +
      `  ${pc.dim('•')} ${pc.green('org-chart.mmd')}  ${pc.dim('- mermaid diagram')}\n` +
      `  ${pc.dim('•')} ${pc.green('README.md')}      ${pc.dim(
        '- mission, values, roster'
      )}\n` +
      `  ${pc.dim('•')} ${pc.green('agents/')}        ${pc.dim(
        '- personnel folders (' + workspace.agentCount + ')'
      )}` +
      seedNote
  );
  clack.outro(
    `Launch with: ${pc.cyan(`cd ${workspace.dirName} && animaos launch "your task"`)}`
  );
}

async function promptRequiredText(input: {
  message: string;
  placeholder: string;
  initialValue?: string;
}): Promise<string> {
  const value = await clack.text({
    message: input.message,
    placeholder: input.placeholder,
    initialValue: input.initialValue,
    validate: (candidate) => {
      if (!candidate.trim()) return 'Required';
      return undefined;
    },
  });

  if (clack.isCancel(value)) cancelAndExit();
  return String(value).trim();
}

async function promptCreateKind(): Promise<CreateKind> {
  const selected = await clack.select({
    message: 'What do you want to create?',
    options: [
      {
        value: 'agent',
        label: 'Agent',
        hint: 'one runnable profile',
      },
      {
        value: 'agency',
        label: 'Agency',
        hint: 'orchestrator plus worker team',
      },
    ],
  });

  if (clack.isCancel(selected)) cancelAndExit();
  return selected as CreateKind;
}

async function promptTeamSize(sizeOption: string | undefined): Promise<number> {
  const flagSize = Number(sizeOption);
  const sizeFromFlag =
    Number.isFinite(flagSize) && flagSize >= TEAM_MIN && flagSize <= TEAM_MAX
      ? Math.floor(flagSize)
      : undefined;

  if (sizeFromFlag) return sizeFromFlag;

  const size = await clack.text({
    message: `Team size (including orchestrator, ${TEAM_MIN}-${TEAM_MAX}):`,
    placeholder: '4',
    initialValue: '4',
    validate: (candidate) => {
      const parsed = Number(candidate);
      if (!Number.isFinite(parsed)) return 'Must be a number';
      if (parsed < TEAM_MIN || parsed > TEAM_MAX) {
        return `Must be between ${TEAM_MIN} and ${TEAM_MAX}`;
      }
      return undefined;
    },
  });

  if (clack.isCancel(size)) cancelAndExit();
  return Number(size);
}

function logResolvedProvider(provider: string, explicitProvider: string | undefined) {
  if (explicitProvider) return;

  if (provider === DETERMINISTIC_PROVIDER) {
    clack.log.info(
      `No remote provider credentials were found; using ${DETERMINISTIC_PROVIDER} local scaffolding.`
    );
    return;
  }

  clack.log.info(`Using configured provider "${provider}" for generation.`);
}

async function generateSeedFiles(input: {
  enabled: boolean | undefined;
  provider: string;
  apiKey?: string;
  model: string;
  agencyName: string;
  agencyDescription: string;
  mission?: string;
  agents: AgentDefinition[];
}): Promise<Map<string, string>> {
  const seedsByName = new Map<string, string>();
  if (!input.enabled) return seedsByName;

  const issue = getCreateGenerationCredentialIssue(input.provider, input.apiKey);
  if (issue) {
    clack.log.warn(`Seed generation skipped. ${issue}`);
    return seedsByName;
  }

  const spinner = clack.spinner();
  spinner.start('Generating starter memories...');
  try {
    const adapter = createAdapter(input.provider, input.apiKey);
    const seeds = await generateAgentSeeds({
      adapter,
      model: input.model,
      agencyName: input.agencyName,
      agencyDescription: input.agencyDescription,
      mission: input.mission,
      agents: input.agents,
    });
    for (const { agentName, entries } of seeds) {
      if (entries.length > 0) {
        seedsByName.set(agentName, JSON.stringify(entries, null, 2));
      }
    }
    const total = [...seedsByName.values()].reduce(
      (count, value) => count + (JSON.parse(value) as unknown[]).length,
      0
    );
    spinner.stop(`Generated ${total} seed memories across ${seedsByName.size} agents`);
  } catch (error) {
    spinner.stop('Seed generation failed (skipping)');
    clack.log.warn(error instanceof Error ? error.message : String(error));
  }

  return seedsByName;
}

async function runCreateAgent(nameArg: string | undefined, opts: CreateOptions) {
  const name =
    nameArg ??
    (await promptRequiredText({
      message: 'Agent name:',
      placeholder: 'research-agent',
    }));
  const description =
    opts.description && opts.description.trim()
      ? opts.description.trim()
      : await promptRequiredText({
          message: 'What should this agent do?',
          placeholder: 'Research medical literature and summarize risks clearly',
        });
  const provider = resolveCreateProvider(opts.provider, opts.apiKey);
  logResolvedProvider(provider, opts.provider);

  if (
    normalizeProvider(opts.provider) &&
    providerRequiresApiKey(provider) &&
    !hasProviderApiKey(provider, opts.apiKey)
  ) {
    clack.log.warn(
      `Provider "${provider}" is not configured yet. The workspace was still created; launch will need ${
        providerKeyEnvNames(provider).join(', ') || 'provider credentials'
      } or --api-key.`
    );
  }

  const config = buildSingleAgentConfig({
    name,
    description,
    provider,
    model: opts.model,
    system: opts.system,
  });
  const allAgentDefs: AgentDefinition[] = [
    { ...config.orchestrator, role: 'orchestrator' },
  ];
  const seedsByName = await generateSeedFiles({
    enabled: opts.seed,
    provider,
    apiKey: opts.apiKey,
    model: opts.model,
    agencyName: config.name,
    agencyDescription: config.description,
    mission: config.mission,
    agents: allAgentDefs,
  });

  const workspace = writeWorkspace(config, allAgentDefs, seedsByName);
  logWorkspaceResult(workspace, 'Agent');
}

async function runCreateAgency(nameArg: string | undefined, opts: CreateOptions) {
  const name =
    nameArg ??
    (await promptRequiredText({
      message: 'Agency name:',
      placeholder: 'content-team',
    }));
  const description =
    opts.description && opts.description.trim()
      ? opts.description.trim()
      : await promptRequiredText({
          message: 'What does this agency do?',
          placeholder: 'Research topics and write high-quality articles',
        });
  const teamSize = await promptTeamSize(opts.size);
  const provider = resolveCreateProvider(opts.provider, opts.apiKey);
  logResolvedProvider(provider, opts.provider);

  const credentialIssue = getCreateGenerationCredentialIssue(provider, opts.apiKey);
  if (credentialIssue) {
    clack.log.error(credentialIssue);
    clack.outro(
      `Try again with: ${pc.cyan(
        `animaos create agency ${workspaceSlug(name)} --provider ${DETERMINISTIC_PROVIDER}`
      )}`
    );
    process.exit(1);
  }

  const spinner = clack.spinner();
  spinner.start(
    provider === DETERMINISTIC_PROVIDER ? 'Scaffolding your team...' : 'Building your team...'
  );

  let agents: AgentDefinition[];
  let mission: string | undefined;
  let values: string[] | undefined;
  try {
    const adapter = createAdapter(provider, opts.apiKey);
    const generated = await generateAgentTeam({
      adapter,
      model: opts.model,
      agencyName: name,
      agencyDescription: description,
      teamSize,
      modelPool: splitModels(opts.models),
    });
    agents = generated.agents;
    mission = generated.mission;
    values = generated.values;
    spinner.stop(`Generated ${agents.length} agents`);
  } catch (error) {
    spinner.stop('Failed to generate team');
    clack.log.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }

  const orchestrator = agents.find((agent) => agent.role === 'orchestrator') ?? agents[0];
  const workers = agents.filter((agent) => agent !== orchestrator);

  if (!orchestrator) {
    clack.log.error('The generator returned no agents.');
    process.exit(1);
  }

  if (mission) {
    clack.log.info(pc.bold('Mission'));
    clack.log.message(`  ${mission}`);
  }
  if (values?.length) {
    clack.log.info(pc.bold('Values'));
    clack.log.message(values.map((value) => `  • ${value}`).join('\n'));
  }

  const formatLine = (agent: AgentDefinition, marker: string) => {
    const position = agent.position ? pc.dim(` - ${agent.position}`) : '';
    const skills = agent.tools?.length
      ? `\n      ${pc.dim('skills:')} ${agent.tools.join(', ')}`
      : '';
    const collab = agent.collaboratesWith?.length
      ? `\n      ${pc.dim('collabs:')} ${agent.collaboratesWith.join(', ')}`
      : '';
    return `  ${marker} ${pc.cyan(agent.name)}${position}\n      ${agent.bio}${skills}${collab}`;
  };

  clack.log.info(pc.bold('Your team:'));
  clack.log.message(formatLine(orchestrator, pc.yellow('★')));
  for (const agent of workers) {
    clack.log.message(formatLine(agent, pc.dim('•')));
  }

  const accepted = opts.yes ? true : await clack.confirm({ message: 'Accept this team?' });
  if (clack.isCancel(accepted) || !accepted) cancelAndExit();

  clack.log.info(pc.dim('You can edit the agents later in anima.yaml'));

  const allAgentDefs: AgentDefinition[] = [
    { ...orchestrator, role: 'orchestrator' },
    ...workers.map((agent) => ({ ...agent, role: 'worker' as const })),
  ];
  const seedsByName = await generateSeedFiles({
    enabled: opts.seed,
    provider,
    apiKey: opts.apiKey,
    model: opts.model,
    agencyName: name,
    agencyDescription: description,
    mission,
    agents: allAgentDefs,
  });

  const config: AgencyConfig = {
    name,
    description,
    mission,
    values,
    model: opts.model,
    provider,
    strategy: 'supervisor',
    orchestrator: toAgentDef(orchestrator),
    agents: workers.map(toAgentDef),
  };

  const workspace = writeWorkspace(config, allAgentDefs, seedsByName);
  logWorkspaceResult(workspace, 'Agency');
}

export async function executeCreateCommand(
  kindOrName: string | undefined,
  nameArg: string | undefined,
  opts: CreateOptions
): Promise<void> {
  clack.intro(pc.bgCyan(pc.black(' animaOS-SWARM ')));

  const target = resolveCreateTarget(kindOrName, nameArg, opts.kind);
  if (target.error) {
    clack.log.error(target.error);
    process.exit(1);
  }

  const providerIssue = getCreateProviderIssue(opts.provider);
  if (providerIssue) {
    clack.log.error(providerIssue);
    process.exit(1);
  }

  const kind = target.kind ?? await promptCreateKind();

  if (kind === 'agent') {
    await runCreateAgent(target.nameArg, opts);
    return;
  }

  await runCreateAgency(target.nameArg, opts);
}

export const createCommand = new Command('create')
  .description('Create a new agent or agency workspace')
  .argument('[kind-or-name]', 'What to create (agent|agency) or legacy agency name')
  .argument('[name]', 'Agent or agency name')
  .option('--kind <kind>', 'What to create: agent or agency')
  .option('-p, --provider <provider>', PROVIDER_HELP_TEXT)
  .option('-m, --model <model>', 'Model to use', DEFAULT_CREATE_MODEL)
  .option('--models <list>', 'Comma-separated model pool to distribute across agency agents')
  .option('-d, --description <description>', 'Agent or agency description')
  .option('-s, --size <number>', 'Agency team size including orchestrator (2-10)', '4')
  .option('--api-key <key>', 'API key')
  .option('--system <prompt>', 'System prompt for agent mode')
  .option('--seed', 'Generate starter memories for each agent using the selected generator')
  .option('-y, --yes', 'Accept the generated agency team without confirmation')
  .action(
    (kindOrName: string | undefined, nameArg: string | undefined, opts: CreateOptions) =>
      executeCreateCommand(kindOrName, nameArg, opts)
  );