# Web Chat Markdown Design

## Goal

Render user and assistant chat messages as polished, safe GitHub-flavored Markdown in the web console at `http://localhost:4200/`. The result should make normal model responses readable without changing the daemon message contract or other chat surfaces.

## Scope

The main workspace conversation will support:

- paragraphs, headings, emphasis, strong text, and strikethrough;
- ordered and unordered lists, task lists, and blockquotes;
- links, inline code, fenced code blocks, and language-aware syntax highlighting;
- tables that remain usable on narrow screens; and
- a copy button for fenced code blocks.

Markdown applies to both `User` and `Assistant` bubbles. System and tool event pills remain compact plain text. Telegram rendering, message persistence, transport types, prompt behavior, attachments, math notation, Mermaid diagrams, and rich media are outside this change.

## Architecture

Add a focused Markdown presentation component under `apps/web/src/components/`. `ChatScreen` will delegate message body rendering to that component while retaining ownership of bubble alignment, color, timestamps, and non-chat event pills.

Use `react-markdown` for React-native rendering, `remark-gfm` for GitHub-flavored syntax, and `prism-react-renderer` for fenced-code highlighting. These libraries avoid a custom parser and do not require injecting generated HTML into the DOM.

The Markdown component will provide element mappings for links, tables, inline code, fenced code blocks, and other prose elements so rendered content follows the existing dark visual system. A dedicated code-block unit will own syntax highlighting, horizontal overflow, the language label, clipboard interaction, and copied-state feedback.

## Safety and Link Behavior

Raw HTML embedded in message text will not be parsed. The implementation will not use `dangerouslySetInnerHTML` or enable a raw-HTML plugin. React Markdown's safe URL transformation will remain in force so unsafe protocols such as `javascript:` do not become executable links.

Absolute HTTP and HTTPS links will open in a new tab with `rel="noopener noreferrer"`. Relative links and safe non-web schemes supported by the renderer remain in the current tab. Copying code is an explicit local clipboard action and does not transmit data.

## Responsive Presentation

Markdown typography will be implemented with scoped Tailwind classes rather than a global prose stylesheet. Adjacent blocks receive compact chat-appropriate spacing. Long words, links, inline code, fenced code, and tables must not expand the conversation beyond its container.

Tables will sit inside a horizontal overflow wrapper. Fenced code blocks will scroll horizontally inside the bubble, while ordinary prose continues to wrap. User and assistant bubbles retain their current width, alignment, border, and background treatments.

## Error Handling

Valid plain text remains valid Markdown and renders as text. Unknown fenced-code languages fall back to unhighlighted code without failing the message. If the Clipboard API rejects a copy request, the button remains available and does not show a successful state; rendering and chat interaction continue normally.

## Testing

Focused component tests will verify:

- Markdown semantics render for both user and assistant messages;
- GFM task lists, tables, and strikethrough are supported;
- raw HTML and unsafe link protocols cannot create executable content;
- safe external links receive the expected target and relationship attributes;
- fenced code exposes its language, highlights when supported, and can be copied;
- inline code is not treated as a fenced code block; and
- long-form Markdown keeps the required overflow wrappers and scoped styling hooks.

Verification will run the web project's focused tests first, followed by its Nx `test`, `typecheck`, and `build` targets with cache disabled. The live localhost page will then be reloaded and inspected with an existing Markdown-heavy conversation.

## Acceptance Criteria

1. Both user and assistant bubbles render the supported Markdown constructs instead of displaying their markers literally.
2. Fenced code blocks are readable, language-aware, horizontally scrollable, and copyable.
3. Tables remain contained on desktop and mobile widths.
4. Raw HTML and unsafe URLs cannot introduce executable markup or links.
5. Existing bubble layout, event pills, timestamps, composer behavior, and daemon data flow remain unchanged.
6. Current web tests, typechecking, and production build pass.
