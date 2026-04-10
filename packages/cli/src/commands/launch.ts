import { Command } from 'commander';
import { writeFile, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Interface } from 'node:readline';
import yaml from 'js-yaml';
import {
  DAEMON_HEALTH_UNAVAILABLE_MESSAGE,
  DAEMON_RECOVERED_MESSAGE,
  describeDaemonWarningTransition,
  formatDaemonUnreachableWarning,
  type AgentConfig,
  type IEventBus,
  type TaskResult,
} from '@animaOS-SWARM/core';
import type { SwarmConfig } from '@animaOS-SWARM/sdk';
import { loadAgency, agencyExists } from '../agency/loader.js';
import { createCliDaemonClient, type CliDaemonClient } from '../client.js';
import type { AgencyConfig, AgentDefinition } from '../agency/types.js';
import type { AgentProfile, ResultEntry } from '@animaOS-SWARM/tui';
import {
  emitLaunchTaskFailure,
  emitLaunchTaskQueued,
  emitLaunchTaskStart,
  launchDisplayAgents,
  relayLaunchSwarmEvent,
} from './launch-events.js';
import {
  appendLaunchHistory,
  clearLaunchHistory,
  loadLaunchHistory,
  saveLaunchHistory,
} from './launch-history.js';
import {
  extractResultText,
  getErrorMessage,
  resolveDaemonModelSettings,
} from './utils.js';

export interface LaunchOptions {
  dir: string;
  apiKey?: string;
  tui: boolean;
}

interface DaemonTuiRuntime {
  eventBus: IEventBus;
  render: (element: any) => {
    unmount: () => void;
    waitUntilExit: () => Promise<unknown>;
  };
  createElement: (
    component: unknown,
    props: Record<string, unknown>
  ) => unknown;
  App: unknown;
}

interface LaunchDeps {
  client?: Pick<CliDaemonClient, 'swarms'> &
    Partial<Pick<CliDaemonClient, 'health'>>;
  createReadline?: () => Pick<Interface, 'question' | 'close'>;
  createDaemonTuiRuntime?: () => Promise<DaemonTuiRuntime>;
}

interface DaemonToolDescriptor {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
}

interface DaemonPluginDescriptor {
  name: string;
  description: string;
}

type DaemonAgentConfig = Omit<AgentConfig, 'tools' | 'plugins'> & {
  tools?: DaemonToolDescriptor[];
  plugins?: DaemonPluginDescriptor[];
};

interface DaemonSwarmConfig extends Omit<SwarmConfig, 'manager' | 'workers'> {
  manager: DaemonAgentConfig;
  workers: DaemonAgentConfig[];
}

type DaemonSwarmSnapshot = Awaited<
  ReturnType<CliDaemonClient['swarms']['create']>
>;

const DEFAULT_DAEMON_TOOL_NAMES = ['memory_search', 'recent_memories'] as const;

const DAEMON_MEMORY_PLUGIN: DaemonPluginDescriptor = {
  name: 'memory',
  description: 'Built-in daemon memory context and reflection support.',
};

const DAEMON_TOOL_ALIASES = new Map<string, string>([
  ['memory_recent', 'recent_memories'],
]);

function plainTextLaunchHelp(hasHealth: boolean): string {
  return hasHealth
    ? 'Commands: /help show available commands · /health recheck daemon connectivity · exit quit'
    : 'Commands: /help show available commands · exit quit';
}

function daemonObjectToolParameters(
  properties: Record<string, unknown>,
  required: string[] = []
): Record<string, unknown> {
  return {
    type: 'object',
    properties,
    ...(required.length > 0 ? { required } : {}),
  };
}

const DAEMON_TOOL_DESCRIPTOR_MAP = new Map<string, DaemonToolDescriptor>([
  [
    'memory_search',
    {
      name: 'memory_search',
      description: 'Search agent memory for relevant facts and prior work.',
      parameters: daemonObjectToolParameters(
        {
          query: { type: 'string' },
          limit: { type: 'number' },
        },
        ['query']
      ),
    },
  ],
  [
    'memory_add',
    {
      name: 'memory_add',
      description: 'Store a new memory entry for the current agent.',
      parameters: daemonObjectToolParameters(
        {
          content: { type: 'string' },
          type: { type: 'string' },
          importance: { type: 'number' },
        },
        ['content']
      ),
    },
  ],
  [
    'recent_memories',
    {
      name: 'recent_memories',
      description: "List the current agent's recent memories.",
      parameters: daemonObjectToolParameters({
        limit: { type: 'number' },
      }),
    },
  ],
]);

function daemonToolName(toolName: string): string {
  return DAEMON_TOOL_ALIASES.get(toolName) ?? toolName;
}

function daemonToolsForAgent(agent: AgentDefinition): DaemonToolDescriptor[] {
  const supportedToolNames = new Set<string>(DEFAULT_DAEMON_TOOL_NAMES);
  const unsupportedTools: string[] = [];

  for (const rawToolName of agent.tools ?? []) {
    const toolName = rawToolName.trim();
    if (!toolName) {
      continue;
    }

    const normalizedToolName = daemonToolName(toolName);
    if (!DAEMON_TOOL_DESCRIPTOR_MAP.has(normalizedToolName)) {
      unsupportedTools.push(toolName);
      continue;
    }

    supportedToolNames.add(normalizedToolName);
  }

  if (unsupportedTools.length > 0) {
    throw new Error(
      `daemon-backed launch does not support tool(s) for agent "${
        agent.name
      }": ${unsupportedTools.join(
        ', '
      )}. Launch now runs only through the Rust daemon; remove those tools from anima.yaml or implement them in the daemon tool registry.`
    );
  }

  return Array.from(supportedToolNames, (toolName) => {
    const descriptor = DAEMON_TOOL_DESCRIPTOR_MAP.get(toolName);
    if (!descriptor) {
      throw new Error(`missing daemon tool descriptor for ${toolName}`);
    }

    return {
      name: descriptor.name,
      description: descriptor.description,
      parameters: descriptor.parameters,
    };
  });
}

function createDaemonSwarmSession(
  client: Pick<CliDaemonClient, 'swarms'>,
  agency: AgencyConfig,
  opts: Pick<LaunchOptions, 'apiKey'>
): {
  getSwarm: () => Promise<DaemonSwarmSnapshot>;
  invalidate: () => void;
} {
  let swarmConfig = buildDaemonSwarmConfig(agency, opts);
  let swarmPromise: Promise<DaemonSwarmSnapshot> | undefined;

  return {
    getSwarm() {
      if (!swarmPromise) {
        swarmPromise = client.swarms
          .create(swarmConfig as unknown as SwarmConfig)
          .catch((error: unknown) => {
            swarmPromise = undefined;
            throw error;
          });
      }

      return swarmPromise;
    },
    invalidate() {
      swarmConfig = buildDaemonSwarmConfig(agency, opts);
      swarmPromise = undefined;
    },
  };
}

async function getDaemonPreflightWarning(
  client: Partial<Pick<CliDaemonClient, 'health'>>
): Promise<string | undefined> {
  if (!client.health) {
    return undefined;
  }

  try {
    await client.health();
    return undefined;
  } catch (error) {
    return formatDaemonUnreachableWarning(getErrorMessage(error));
  }
}

function createDaemonWarningPoller(
  client: Partial<Pick<CliDaemonClient, 'health'>>
): (() => Promise<string | undefined>) | undefined {
  if (!client.health) {
    return undefined;
  }

  return () => getDaemonPreflightWarning(client);
}

function printPlainTextDaemonWarning(warning: string) {
  console.error('Warning:', warning);
}

function printPlainTextDaemonRecovery() {
  console.log(DAEMON_RECOVERED_MESSAGE);
}

function printPlainTextDaemonHealthy() {
  const transition = describeDaemonWarningTransition(null, null, 'manual');
  if (transition.message) {
    console.log(transition.message);
  }
}

function printPlainTextDaemonHealthUnavailable() {
  console.log(DAEMON_HEALTH_UNAVAILABLE_MESSAGE);
}

function printPlainTextLaunchHelp(hasHealth: boolean) {
  console.log(plainTextLaunchHelp(hasHealth));
}

function agentDefToDaemonConfig(
  agent: AgentDefinition,
  defaultModel: string,
  provider: string,
  settings?: AgentConfig['settings']
): DaemonAgentConfig {
  return {
    name: agent.name,
    bio: agent.bio,
    lore: agent.lore,
    adjectives: agent.adjectives,
    topics: agent.topics,
    knowledge: agent.knowledge,
    style: agent.style,
    model: agent.model ?? defaultModel,
    provider,
    system: agent.system,
    tools: daemonToolsForAgent(agent),
    plugins: [DAEMON_MEMORY_PLUGIN],
    settings,
  };
}

function saveAgency(dir: string, agency: AgencyConfig) {
  writeFileSync(
    join(dir, 'anima.yaml'),
    yaml.dump(agency, { lineWidth: 120, noRefs: true })
  );
}

function buildDaemonSwarmConfig(
  agency: AgencyConfig,
  opts: Pick<LaunchOptions, 'apiKey'>
): DaemonSwarmConfig {
  const settings = resolveDaemonModelSettings(agency.provider, opts.apiKey);

  return {
    strategy: agency.strategy,
    ...(agency.maxParallelDelegations
      ? { maxParallelDelegations: agency.maxParallelDelegations }
      : {}),
    manager: agentDefToDaemonConfig(
      agency.orchestrator,
      agency.model,
      agency.provider,
      settings
    ),
    workers: agency.agents.map((agent) =>
      agentDefToDaemonConfig(agent, agency.model, agency.provider, settings)
    ),
  };
}

function resultText(result: TaskResult): string {
  if (result.status !== 'success') {
    return `Error: ${result.error}`;
  }

  return extractResultText(result) ?? JSON.stringify(result.data);
}

function historyEntry(task: string, result: TaskResult): ResultEntry {
  return {
    id: `run-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    timestamp: Date.now(),
    task,
    result: resultText(result),
    isError: result.status !== 'success',
  };
}

function printLaunchResult(result: TaskResult, setExitCodeOnError = false) {
  console.log('\n--- Result ---');
  if (result.status === 'success') {
    console.log(resultText(result));
  } else {
    console.error('Error:', result.error);
    if (setExitCodeOnError) {
      process.exitCode = 1;
    }
  }
  console.log(`\nDuration: ${result.durationMs}ms`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function loadDaemonTuiRuntime(): Promise<DaemonTuiRuntime> {
  const [{ EventBus }, { render }, { default: React }, { App }] =
    await Promise.all([
      import('@animaOS-SWARM/core'),
      import('ink'),
      import('react'),
      import('@animaOS-SWARM/tui'),
    ]);

  return {
    eventBus: new EventBus() as IEventBus,
    render,
    createElement: React.createElement,
    App,
  };
}

async function executeDaemonLaunchCommand(
  task: string | undefined,
  opts: LaunchOptions,
  agency: AgencyConfig,
  deps: LaunchDeps
): Promise<void> {
  try {
    const client = deps.client ?? createCliDaemonClient();
    const swarmSession = createDaemonSwarmSession(client, agency, opts);
    let daemonWarning = await getDaemonPreflightWarning(client);

    if (!task) {
      const { createInterface } = await import('node:readline');
      const rl =
        deps.createReadline?.() ??
        createInterface({
          input: process.stdin,
          output: process.stdout,
        });

      console.log(
        `${agency.name} — ${agency.strategy} strategy — ${agency.model}`
      );
      if (daemonWarning) {
        printPlainTextDaemonWarning(daemonWarning);
      }
      console.log(
        client.health
          ? 'Type "exit" to quit. Type "/health" to recheck daemon connectivity.\n'
          : 'Type "exit" to quit.\n'
      );

      await new Promise<void>((resolve) => {
        const prompt = () => {
          rl.question('task > ', async (input) => {
            const trimmed = input.trim();
            if (!trimmed || trimmed === 'exit') {
              console.log('Bye.');
              rl.close();
              resolve();
              return;
            }

            if (trimmed === '/health') {
              if (!client.health) {
                printPlainTextDaemonHealthUnavailable();
              } else {
                const nextWarning = await getDaemonPreflightWarning(client);
                const transition = describeDaemonWarningTransition(
                  daemonWarning,
                  nextWarning,
                  'manual'
                );
                if (transition.message) {
                  if (nextWarning) {
                    printPlainTextDaemonWarning(nextWarning);
                  } else if (transition.recovered) {
                    printPlainTextDaemonRecovery();
                  } else {
                    printPlainTextDaemonHealthy();
                  }
                }
                daemonWarning = nextWarning;
              }

              console.log();
              prompt();
              return;
            }

            if (trimmed === '/help') {
              printPlainTextLaunchHelp(Boolean(client.health));
              console.log();
              prompt();
              return;
            }

            try {
              const swarm = await swarmSession.getSwarm();
              const execution = await client.swarms.run(swarm.id, {
                text: trimmed,
              });
              if (daemonWarning) {
                printPlainTextDaemonRecovery();
                daemonWarning = undefined;
              }
              appendLaunchHistory(
                opts.dir,
                historyEntry(trimmed, execution.result)
              );
              printLaunchResult(execution.result);
            } catch (error) {
              swarmSession.invalidate();
              console.error('Error:', getErrorMessage(error));
              const nextWarning = await getDaemonPreflightWarning(client);
              if (nextWarning && nextWarning !== daemonWarning) {
                printPlainTextDaemonWarning(nextWarning);
              }
              daemonWarning = nextWarning;
            }

            console.log();
            prompt();
          });
        };

        prompt();
      });
      return;
    }

    if (daemonWarning) {
      printPlainTextDaemonWarning(daemonWarning);
    }
    const swarm = await swarmSession.getSwarm();
    console.log(
      `Launching "${agency.name}" with strategy "${agency.strategy}" and model ${agency.model}...\n`
    );
    const execution = await client.swarms.run(swarm.id, { text: task });
    if (daemonWarning) {
      printPlainTextDaemonRecovery();
      daemonWarning = undefined;
    }
    appendLaunchHistory(opts.dir, historyEntry(task, execution.result));
    printLaunchResult(execution.result, true);
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
  }
}

function toProfile(
  a: AgentDefinition,
  role: AgentProfile['role']
): AgentProfile {
  return {
    name: a.name,
    role,
    bio: a.bio,
    lore: a.lore,
    adjectives: a.adjectives,
    topics: a.topics,
    knowledge: a.knowledge,
    style: a.style,
    system: a.system,
  };
}

async function executeDaemonTuiLaunchCommand(
  task: string | undefined,
  opts: LaunchOptions,
  agency: AgencyConfig,
  deps: LaunchDeps
): Promise<void> {
  try {
    const client = deps.client ?? createCliDaemonClient();
    const swarmSession = createDaemonSwarmSession(client, agency, opts);
    const tuiRuntime = deps.createDaemonTuiRuntime
      ? await deps.createDaemonTuiRuntime()
      : await loadDaemonTuiRuntime();
    const bus = tuiRuntime.eventBus;
    const initialResults = loadLaunchHistory(opts.dir);
    const preflightWarning = await getDaemonPreflightWarning(client);
    const pollDaemonWarning = createDaemonWarningPoller(client);

    const agentProfiles: AgentProfile[] = [
      toProfile(agency.orchestrator, 'orchestrator'),
      ...agency.agents.map((agent) => toProfile(agent, 'worker')),
    ];

    function onSaveAgent(profile: AgentProfile) {
      if (profile.name === agency.orchestrator.name) {
        Object.assign(agency.orchestrator, profile);
      } else {
        const idx = agency.agents.findIndex(
          (agent) => agent.name === profile.name
        );
        if (idx >= 0) Object.assign(agency.agents[idx], profile);
      }
      saveAgency(opts.dir, agency);
      swarmSession.invalidate();
    }

    const runTask = async (input: string): Promise<TaskResult> => {
      const displayAgents = launchDisplayAgents(agency);
      let abortController: AbortController | undefined;
      let subscription: Promise<void> | undefined;
      let sawCompletion = false;
      let subscriptionError: string | undefined;
      let launchStarted = false;

      try {
        const swarm = await swarmSession.getSwarm();
        abortController = new AbortController();
        subscription = (async () => {
          try {
            for await (const event of client.swarms.subscribe(swarm.id, {
              signal: abortController.signal,
            })) {
              await relayLaunchSwarmEvent(bus, displayAgents, event);
              if (event.event === 'swarm:completed') {
                sawCompletion = true;
                break;
              }
            }
          } catch (error) {
            if (!abortController?.signal.aborted) {
              subscriptionError = getErrorMessage(error);
            }
          }
        })();

        await emitLaunchTaskStart(bus, displayAgents, input);
        launchStarted = true;
        const execution = await client.swarms.run(swarm.id, { text: input });
        await Promise.race([subscription, sleep(2000)]);
        abortController.abort();
        await subscription;

        if (!sawCompletion) {
          if (subscriptionError) {
            console.error('Warning:', subscriptionError);
          }
          await relayLaunchSwarmEvent(bus, displayAgents, {
            event: 'swarm:completed',
            data: {
              swarmId: swarm.id,
              state: execution.swarm,
              result: execution.result,
            },
          });
        }

        const text = resultText(execution.result);
        appendLaunchHistory(opts.dir, historyEntry(input, execution.result));
        writeFile(
          join(opts.dir, 'anima-result.md'),
          `# Task\n\n${input}\n\n# Result\n\n${text}\n`,
          () => {}
        );
        return execution.result;
      } catch (error) {
        swarmSession.invalidate();
        abortController?.abort();
        await subscription;
        const message = getErrorMessage(error);
        if (!launchStarted) {
          await emitLaunchTaskQueued(bus, displayAgents, input);
        }
        await emitLaunchTaskFailure(bus, displayAgents, message);
        return {
          status: 'error',
          error: message,
          durationMs: 0,
        };
      }
    };

    if (!task) {
      const element = tuiRuntime.createElement(tuiRuntime.App, {
        eventBus: bus,
        strategy: agency.strategy,
        interactive: true,
        onTask: runTask,
        agentProfiles,
        onSaveAgent,
        initialResults,
        resumeLastResult: initialResults.length > 0,
        onResultRecorded: () => undefined,
        onHistoryUpdated: (entries: ResultEntry[]) =>
          saveLaunchHistory(opts.dir, entries),
        onClearHistory: () => clearLaunchHistory(opts.dir),
        preflightWarning,
        pollDaemonWarning,
      });
      tuiRuntime.render(element);
      return;
    }

    const element = tuiRuntime.createElement(tuiRuntime.App, {
      eventBus: bus,
      strategy: agency.strategy,
      task,
      onTask: runTask,
      agentProfiles,
      onSaveAgent,
      initialResults,
      onResultRecorded: () => undefined,
      onHistoryUpdated: (entries: ResultEntry[]) =>
        saveLaunchHistory(opts.dir, entries),
      onClearHistory: () => clearLaunchHistory(opts.dir),
      preflightWarning,
      pollDaemonWarning,
    });
    const instance = tuiRuntime.render(element);

    const result = await runTask(task);

    if (result.status === 'error') {
      process.exitCode = 1;
    }

    await instance.waitUntilExit();
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
  }
}

export async function executeLaunchCommand(
  task: string | undefined,
  opts: LaunchOptions,
  deps: LaunchDeps = {}
): Promise<void> {
  if (!agencyExists(opts.dir)) {
    console.error(
      `Error: No anima.yaml found in "${opts.dir}". Run "animaos create" first.`
    );
    process.exitCode = 1;
    return;
  }

  let agency: AgencyConfig;
  try {
    agency = loadAgency(opts.dir);
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
    return;
  }

  if (!opts.tui) {
    await executeDaemonLaunchCommand(task, opts, agency, deps);
    return;
  }

  await executeDaemonTuiLaunchCommand(task, opts, agency, deps);
}

export const launchCommand = new Command('launch')
  .description('Launch an agent swarm from an anima.yaml config')
  .argument(
    '[task]',
    'The task to execute (omit to open interactive TUI session)'
  )
  .option('-d, --dir <dir>', 'Directory containing anima.yaml', '.')
  .option('--api-key <key>', 'API key override')
  .option('--no-tui', 'Disable TUI, use plain text output')
  .action((task: string | undefined, opts: LaunchOptions) =>
    executeLaunchCommand(task, opts)
  );
