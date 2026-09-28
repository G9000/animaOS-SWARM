import type { SlashCommand } from '../../lib/slash-commands';

/** The composer's command list (spec §15.3); the input keeps focus and
 *  moves through it with the arrow keys. */
export function SlashCommandMenu({
  id,
  commands,
  activeName,
  onPick,
}: {
  id: string;
  commands: readonly SlashCommand[];
  activeName: string | null;
  onPick: (command: SlashCommand) => void;
}) {
  return (
    <ul id={id} role="listbox" aria-label="Commands" className="slash-menu">
      {commands.map((command) => (
        <li
          key={command.name}
          id={`${id}-${command.name}`}
          role="option"
          aria-selected={command.name === activeName}
          className="slash-menu-option"
          onMouseDown={(event) => {
            // Keep focus in the input.
            event.preventDefault();
            onPick(command);
          }}
        >
          <span className="slash-menu-name">
            /{command.name}
            {command.placeholder ? ` ${command.placeholder}` : ''}
          </span>
          <span className="slash-menu-description">{command.description}</span>
        </li>
      ))}
    </ul>
  );
}
