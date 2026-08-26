import { Command } from 'commander';
import { createInterface, type Interface } from 'node:readline';
import { createCliDaemonClient, type CliDaemonClient } from '../client.js';
import { getErrorMessage } from './utils.js';

export interface AssistantChatOptions {
  name: string;
  userId?: string;
  userName?: string;
}

export interface AssistantInboxOptions {
  since?: string;
  limit?: string;
}

interface AssistantChatDeps {
  client?: Pick<CliDaemonClient, 'agents'>;
  createReadline?: () => Pick<Interface, 'question' | 'close'>;
}

interface AssistantInboxDeps {
  client?: Pick<CliDaemonClient, 'requestJson'>;
}

export interface AssistantOutboxMessage {
  id: string;
  job: string;
  text: string;
  createdAtMs: number;
}

function resolveUserName(explicit?: string): string {
  return (
    explicit?.trim() ||
    process.env.ANIMAOS_USER_NAME ||
    process.env.USERNAME ||
    process.env.USER ||
    'unknown'
  );
}

function resolveUserId(explicit?: string): string {
  return explicit?.trim() || process.env.ANIMAOS_USER_ID || resolveUserName();
}

export async function executeAssistantChatCommand(
  opts: AssistantChatOptions,
  deps: AssistantChatDeps = {}
): Promise<void> {
  const client = deps.client ?? createCliDaemonClient();
  const userId = resolveUserId(opts.userId);
  const userName = resolveUserName(opts.userName);

  let agent: Awaited<ReturnType<CliDaemonClient['agents']['list']>>[number];

  try {
    const agents = await client.agents.list();
    const found = agents.find((entry) => entry.state.name === opts.name);

    if (!found) {
      console.error(
        `Error: no agent named "${opts.name}" found on the daemon. ` +
          'Start the daemon with ANIMAOS_RS_ASSISTANT_ENABLED=1 to enable the assistant.'
      );
      process.exitCode = 1;
      return;
    }

    agent = found;
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
    return;
  }

  console.log(`AnimaOS Kit - assistant (${agent.state.name})`);
  console.log('Type "exit" to quit.\n');

  const rl =
    deps.createReadline?.() ??
    createInterface({
      input: process.stdin,
      output: process.stdout,
    });

  await new Promise<void>((resolve) => {
    const prompt = () => {
      rl.question('you > ', async (input) => {
        const trimmed = input.trim();
        if (!trimmed || trimmed === 'exit') {
          console.log('Bye.');
          rl.close();
          resolve();
          return;
        }

        try {
          const result = await client.agents.run(agent.state.id, {
            text: trimmed,
            metadata: { userId, userName },
          });

          if (result.result.status === 'success') {
            const text =
              typeof result.result.data === 'object' &&
              result.result.data !== null &&
              'text' in result.result.data
                ? result.result.data.text
                : JSON.stringify(result.result.data);
            console.log(`\nagent > ${text}\n`);
          } else {
            console.log(`\n[error] ${result.result.error}\n`);
          }
        } catch (error) {
          console.log(`\n[error] ${getErrorMessage(error)}\n`);
        }

        prompt();
      });
    };

    prompt();
  });
}

export async function executeAssistantInboxCommand(
  opts: AssistantInboxOptions,
  deps: AssistantInboxDeps = {}
): Promise<void> {
  const client = deps.client ?? createCliDaemonClient();

  const search = new URLSearchParams();
  if (opts.since !== undefined) {
    search.set('since', opts.since);
  }
  if (opts.limit !== undefined) {
    search.set('limit', opts.limit);
  }

  const path = search.size
    ? `/api/assistant/outbox?${search.toString()}`
    : '/api/assistant/outbox';

  try {
    const response = await client.requestJson<{
      messages: AssistantOutboxMessage[];
    }>(path);

    if (response.messages.length === 0) {
      console.log('No new messages.');
      return;
    }

    for (const message of response.messages) {
      const time = new Date(message.createdAtMs).toISOString();
      console.log(`[${time}] (${message.job}) ${message.text}`);
    }
  } catch (error) {
    console.error('Error:', getErrorMessage(error));
    process.exitCode = 1;
  }
}

const chatSubcommand = new Command('chat')
  .description('Interactive chat with the persistent assistant agent')
  .option('-n, --name <name>', 'Assistant agent name', 'assistant')
  .option('--user-id <id>', 'User id sent with each run (default: $ANIMAOS_USER_ID or OS username)')
  .option('--user-name <name>', 'User name sent with each run (default: $ANIMAOS_USER_NAME or OS username)')
  .action((opts: AssistantChatOptions) => executeAssistantChatCommand(opts));

const inboxSubcommand = new Command('inbox')
  .description('Show proactive messages from the assistant outbox')
  .option('--since <ms>', 'Only messages created after this epoch millis timestamp')
  .option('--limit <n>', 'Maximum number of messages to fetch')
  .action((opts: AssistantInboxOptions) => executeAssistantInboxCommand(opts));

export const assistantCommand = new Command('assistant')
  .description('Interact with the persistent assistant agent')
  .addCommand(chatSubcommand, { isDefault: true })
  .addCommand(inboxSubcommand);
