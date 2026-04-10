import type { SlashCommand } from './components/input-bar.js';

const SWARM_SLASH_COMMANDS: SlashCommand[] = [
  { name: 'agents', description: 'browse and edit agents' },
  { name: 'history', description: 'browse past runs' },
  { name: 'resume', description: 'browse saved runs or resume by label' },
  { name: 'rename', description: 'name the current saved run' },
  {
    name: 'delete',
    description: 'delete a saved run by label',
    args: '<label>',
  },
  { name: 'undo', description: 'restore the last deleted saved run' },
  { name: 'undo-drop', description: 'discard the oldest queued undo' },
  { name: 'undo-status', description: 'show deleted-run undo queue' },
  { name: 'trace', description: 'inspect messages and tool activity' },
  { name: 'result', description: 'view the full last result' },
  { name: 'status', description: 'show current session state' },
  { name: 'health', description: 'check current daemon connectivity' },
  { name: 'retry', description: 'rerun the last task' },
  { name: 'help', description: 'show available commands' },
  { name: 'clear', description: 'clear session history' },
  { name: 'exit', description: 'exit the session' },
];

const RESULT_BASE_COMMANDS: SlashCommand[] = [
  { name: 'back', description: 'return to swarm view' },
  {
    name: 'resume',
    description: 'browse saved runs to resume',
  },
  {
    name: 'rename',
    description: 'name the current saved run',
    args: '<label>',
  },
  {
    name: 'delete',
    description: 'delete the current saved run',
  },
  {
    name: 'undo',
    description: 'restore the last deleted saved run',
  },
  {
    name: 'undo-drop',
    description: 'discard the oldest queued undo',
  },
  {
    name: 'undo-status',
    description: 'show deleted-run undo queue',
  },
];

const RETRY_COMMAND: SlashCommand = {
  name: 'retry',
  description: 'rerun the last task',
};

const HEALTH_COMMANDS: SlashCommand[] = [
  {
    name: 'health',
    description: 'check current daemon connectivity',
  },
  {
    name: 'help',
    description: 'show available commands',
  },
];

function withoutRetryCommand(commands: SlashCommand[]): SlashCommand[] {
  return commands.filter((command) => command.name !== 'retry');
}

export interface BuildAppInputCommandsOptions {
  interactive: boolean;
  hasSavedRuns: boolean;
  taskEntryBlockedByDaemon: boolean;
  showResultCommands: boolean;
}

export interface AppInputCommands {
  swarmInputCommands: SlashCommand[];
  resultInputCommands: SlashCommand[];
  helpCommands: SlashCommand[];
}

export function buildAppInputCommands({
  interactive,
  hasSavedRuns,
  taskEntryBlockedByDaemon,
  showResultCommands,
}: BuildAppInputCommandsOptions): AppInputCommands {
  const swarmInputCommands = taskEntryBlockedByDaemon
    ? withoutRetryCommand(SWARM_SLASH_COMMANDS)
    : SWARM_SLASH_COMMANDS;
  const resultInputCommands = [
    RESULT_BASE_COMMANDS[0],
    ...(hasSavedRuns ? RESULT_BASE_COMMANDS.slice(1) : []),
    ...(interactive && !taskEntryBlockedByDaemon ? [RETRY_COMMAND] : []),
    ...(taskEntryBlockedByDaemon ? HEALTH_COMMANDS : []),
  ];

  return {
    swarmInputCommands,
    resultInputCommands,
    helpCommands: showResultCommands ? resultInputCommands : swarmInputCommands,
  };
}
