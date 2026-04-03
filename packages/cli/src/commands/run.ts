import { Command } from 'commander';
import { createCliDaemonClient, type CliDaemonClient } from '../client.js';
import type { SwarmConfig } from '@animaOS-SWARM/sdk';
import { getErrorMessage } from './utils.js';

export interface RunOptions {
  model: string;
  provider: string;
  name: string;
  strategy?: 'supervisor' | 'dynamic' | 'round-robin';
  apiKey?: string;
  tui: boolean;
}

export async function executeRunCommand(
  task: string,
  opts: RunOptions,
  client: CliDaemonClient = createCliDaemonClient()
): Promise<void> {
  if (opts.apiKey) {
    console.error(
      'Error:',
      '--api-key is not supported by the daemon-backed run command. Configure credentials in the daemon environment.'
    );
    process.exitCode = 1;
    return;
  }

  if (opts.tui) {
    console.log(
      'TUI mode is not available for daemon-backed runs yet. Falling back to plain text.\n'
    );
  }

  try {
    if (opts.strategy) {
      await runSwarm(task, opts, client);
    } else {
      await runSingleAgent(task, opts, client);
    }
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
  }
}

async function runSingleAgent(
  task: string,
  opts: RunOptions,
  client: Pick<CliDaemonClient, 'agents'>
): Promise<void> {
  const agent = await client.agents.create({
    name: opts.name,
    model: opts.model,
    provider: opts.provider,
    system: 'You are a helpful task agent. Use tools when needed. Be concise.',
  });
  const execution = await client.agents.run(agent.state.id, { text: task });

  console.log(`Agent "${opts.name}" running with ${opts.model}...\n`);
  console.log('--- Result ---');
  if (execution.result.status === 'success') {
    const output =
      typeof execution.result.data === 'object' &&
      execution.result.data !== null &&
      'text' in execution.result.data
        ? execution.result.data.text
        : JSON.stringify(execution.result.data);
    console.log(output);
  } else {
    console.error('Error:', execution.result.error);
    process.exitCode = 1;
  }

  const duration = execution.result.durationMs ?? 0;
  console.log(
    `\nDuration: ${duration}ms | Tokens: ${execution.agent.state.tokenUsage.totalTokens}`
  );
}

async function runSwarm(
  task: string,
  opts: RunOptions,
  client: Pick<CliDaemonClient, 'swarms'>
): Promise<void> {
  const swarm = await client.swarms.create(buildRunSwarmConfig(opts));
  const execution = await client.swarms.run(swarm.id, { text: task });

  console.log(
    `Swarm running with strategy "${opts.strategy}" and model ${opts.model}...\n`
  );
  console.log('--- Result ---');
  if (execution.result.status === 'success') {
    const output =
      typeof execution.result.data === 'object' &&
      execution.result.data !== null &&
      'text' in execution.result.data
        ? execution.result.data.text
        : JSON.stringify(execution.result.data);
    console.log(output);
  } else {
    console.error('Error:', execution.result.error);
    process.exitCode = 1;
  }

  const duration = execution.result.durationMs ?? 0;
  console.log(
    `\nDuration: ${duration}ms | Tokens: ${execution.swarm.tokenUsage.totalTokens}`
  );
}

function buildRunSwarmConfig(opts: RunOptions): SwarmConfig {
  return {
    strategy: opts.strategy!,
    manager: {
      name: 'manager',
      model: opts.model,
      provider: opts.provider,
      system:
        'You are a task manager. Break complex tasks into subtasks and delegate to workers. Synthesize results into a final answer.',
    },
    workers: [
      {
        name: 'worker',
        model: opts.model,
        provider: opts.provider,
        system:
          'You are a helpful worker agent. Complete the assigned task concisely and accurately.',
      },
    ],
  };
}

export const runCommand = new Command('run')
  .description('Run an agent or swarm with a task')
  .argument('<task>', 'The task to execute')
  .option('-m, --model <model>', 'Model to use', 'gpt-4o-mini')
  .option(
    '-p, --provider <provider>',
    'Provider: openai, anthropic, ollama',
    'openai'
  )
  .option('-n, --name <name>', 'Agent name (single-agent mode)', 'task-agent')
  .option(
    '-s, --strategy <strategy>',
    'Swarm strategy: supervisor, dynamic, round-robin'
  )
  .option(
    '--api-key <key>',
    'API key override (unsupported with daemon-backed execution)'
  )
  .option('--no-tui', 'Disable TUI, use plain text output')
  .action((task: string, opts: RunOptions) => executeRunCommand(task, opts));
