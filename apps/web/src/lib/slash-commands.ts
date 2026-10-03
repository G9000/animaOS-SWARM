import type { Skill } from '@animaOS-SWARM/sdk';

/** Composer slash commands (spec §15.3). `/usage` arrives with the Usage
 *  page (M8); `/<skill>` comes from the owner's skills (M5). */
export type SlashCommandName =
  | 'new'
  | 'stop'
  | 'rename'
  | 'archive'
  | 'export'
  | 'search'
  | 'model'
  | 'compact'
  | 'help';

export interface SlashCommand {
  /** A built-in command's name, or a skill's slug. */
  name: string;
  description: string;
  /** Set when the command needs text after its name, e.g. `a title`. */
  needs?: string;
  /** How the menu shows that text, e.g. `<title>`. */
  placeholder?: string;
  /** Set on `/<skill-slug>`: the message is sent with this skill. */
  skill?: string;
}

/** A skill's description in the menu, at most this many characters. */
export const MAX_SKILL_COMMAND_DESCRIPTION = 80;
/** A `/skill` message in a Telegram session (the daemon refuses it too). */
export const SKILL_NOT_IN_TELEGRAM_CHAT =
  'Skills can’t be used in a Telegram chat.';

export const SLASH_COMMANDS: readonly (SlashCommand & {
  name: SlashCommandName;
})[] = [
  { name: 'new', description: 'Start a new chat' },
  { name: 'stop', description: 'Stop the reply in progress' },
  {
    name: 'rename',
    description: 'Rename this chat',
    needs: 'a title',
    placeholder: '<title>',
  },
  { name: 'archive', description: 'Archive or unarchive this chat' },
  { name: 'export', description: 'Download this chat as Markdown' },
  {
    name: 'search',
    description: 'Search your chats',
    needs: 'words to find',
    placeholder: '<words>',
  },
  { name: 'model', description: 'Choose the model in Settings' },
  {
    name: 'compact',
    description: 'Summarize earlier messages to make room',
  },
  { name: 'help', description: 'Show every command' },
];

/** `/<slug>` for each enabled, active skill that does not shadow a
 *  built-in command (spec §15.3). */
export function skillSlashCommands(
  skills: readonly Skill[],
  builtins: readonly SlashCommand[] = SLASH_COMMANDS,
): SlashCommand[] {
  const taken = new Set(builtins.map((command) => command.name));
  return skills
    .filter(
      (skill) =>
        skill.enabled && skill.status === 'active' && !taken.has(skill.slug),
    )
    .map((skill) => ({
      name: skill.slug,
      description: skill.description.slice(0, MAX_SKILL_COMMAND_DESCRIPTION),
      placeholder: '<request>',
      skill: skill.slug,
    }));
}

export interface ParsedSlashCommand {
  command: SlashCommand;
  /** The text after the command's name, trimmed. */
  argument: string;
}

/** The command `text` starts with, or null to send it as a message: text
 *  that matches no command is an ordinary message (spec §15.3). */
export function parseSlashCommand(
  text: string,
  commands: readonly SlashCommand[] = SLASH_COMMANDS,
): ParsedSlashCommand | null {
  const match = /^\/([a-z0-9][a-z0-9-]*)(?:\s+([\s\S]*))?$/.exec(text.trim());
  if (!match) return null;
  const command = commands.find((item) => item.name === match[1]);
  return command ? { command, argument: (match[2] ?? '').trim() } : null;
}

/** The commands matching the first word while only it is typed. */
export function slashSuggestions(
  draft: string,
  commands: readonly SlashCommand[] = SLASH_COMMANDS,
): SlashCommand[] {
  const match = /^\/([a-z0-9-]*)$/.exec(draft);
  if (!match) return [];
  return commands.filter((item) => item.name.startsWith(match[1]));
}

/** What each command does in the open session; a missing handler means the
 *  command is not available there. */
export type SlashCommandHandlers = Partial<
  Record<SlashCommandName, (argument: string) => void>
>;

/** Runs a built-in command: null when it ran, otherwise why it could not.
 *  A skill is sent as a message by the caller, never run here. */
export function runSlashCommand(
  parsed: ParsedSlashCommand,
  handlers: SlashCommandHandlers,
): string | null {
  const { command, argument } = parsed;
  const handler = command.skill
    ? undefined
    : handlers[command.name as SlashCommandName];
  if (!handler) return `/${command.name} is not available here.`;
  if (command.needs && !argument)
    return `Add ${command.needs} after /${command.name}.`;
  // An argument-less command with text after it (S3b-D): refused, not
  // silently ignored, so the extra text is never discarded.
  if (!command.needs && argument) return `/${command.name} takes no text.`;
  handler(argument);
  return null;
}
