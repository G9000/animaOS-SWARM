import { getErrorMessage } from './utils.js';
import { Command } from 'commander';
import pc from 'picocolors';
import {
  createCliDaemonClient,
  type AgentSnapshot,
  type CliDaemonClient,
} from '../client.js';

function renderAgentSummary(agent: AgentSnapshot) {
  console.log();
  console.log(
    `  ${pc.cyan(pc.bold(agent.state.name))} ${pc.dim(`(${agent.state.id})`)}`
  );
  console.log(`    ${pc.dim('status:')} ${agent.state.status}`);
  console.log(`    ${pc.dim('model:')} ${agent.state.config.model}`);
  console.log(
    `    ${pc.dim('messages:')} ${agent.messageCount} ${pc.dim('events:')} ${
      agent.eventCount
    }`
  );
}

function renderAgentDetails(agent: AgentSnapshot) {
  console.log();
  console.log(
    pc.bold(pc.cyan(`  ${agent.state.name}`)) + pc.dim(` (${agent.state.id})`)
  );
  console.log();
  console.log(`  ${pc.bold('Status')}`);
  console.log(`    ${agent.state.status}`);
  console.log();
  console.log(`  ${pc.bold('Model')}`);
  console.log(`    ${agent.state.config.model}`);
  console.log();
  console.log(`  ${pc.bold('Messages')}`);
  console.log(`    ${agent.messageCount}`);
  console.log();
  console.log(`  ${pc.bold('Events')}`);
  console.log(`    ${agent.eventCount}`);
  console.log();
  console.log(`  ${pc.bold('Tokens')}`);
  console.log(`    ${agent.state.tokenUsage.totalTokens}`);

  if (agent.lastTask) {
    console.log();
    console.log(`  ${pc.bold('Last Task')}`);
    console.log(`    ${agent.lastTask.status}`);
  }
}

export async function executeAgentsListCommand(
  client?: Pick<CliDaemonClient, 'agents'>
): Promise<void> {
  try {
    const daemonClient = client ?? createCliDaemonClient();
    const agents = await daemonClient.agents.list();

    if (agents.length === 0) {
      console.log('No daemon-backed agents are currently registered.');
      return;
    }

    console.log(
      pc.bold(
        `\n  ${agents.length} daemon agent${agents.length === 1 ? '' : 's'}`
      )
    );
    for (const agent of agents) {
      renderAgentSummary(agent);
    }
    console.log();
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
  }
}

export async function executeAgentsShowCommand(
  nameOrId: string,
  client?: Pick<CliDaemonClient, 'agents'>
): Promise<void> {
  try {
    const daemonClient = client ?? createCliDaemonClient();
    const agents = await daemonClient.agents.list();
    const match = agents.find(
      (agent) => agent.state.id === nameOrId || agent.state.name === nameOrId
    );

    if (!match) {
      const knownAgents = agents
        .map((agent) => `${agent.state.name} (${agent.state.id})`)
        .join(', ');
      console.error(
        `Error: Agent "${nameOrId}" not found. Available: ${knownAgents}`
      );
      process.exitCode = 1;
      return;
    }

    const snapshot = await daemonClient.agents.get(match.state.id);
    renderAgentDetails(snapshot);
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
  }
}

const listCommand = new Command('list')
  .description('List daemon-backed agents')
  .action(() => executeAgentsListCommand());

const showCommand = new Command('show')
  .description('Show full details of a daemon-backed agent')
  .argument('<name-or-id>', 'Agent name or id')
  .action((nameOrId: string) => executeAgentsShowCommand(nameOrId));

export const agentsCommand = new Command('agents')
  .description('Inspect daemon-backed agents')
  .addCommand(listCommand)
  .addCommand(showCommand);
