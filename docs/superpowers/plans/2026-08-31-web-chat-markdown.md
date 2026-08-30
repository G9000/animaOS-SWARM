# Web Chat Markdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render safe, responsive GitHub-flavored Markdown with highlighted, copyable code blocks in both user and assistant bubbles in the web workspace chat.

**Architecture:** Add one focused `MarkdownMessage` presentation component that owns Markdown parsing, element styling, safe links, tables, and code blocks. Keep `ChatScreen` responsible for roles, bubble layout, timestamps, and event pills, delegating only `User` and `Assistant` message bodies to the new component.

**Tech Stack:** React 19, TypeScript, `react-markdown`, `remark-gfm`, `prism-react-renderer`, Tailwind CSS 4, Testing Library, Vitest 4, Nx 22, Bun.

---

## File Structure

- Create `apps/web/src/components/MarkdownMessage.tsx`: render safe GFM content and own highlighted code/copy behavior.
- Create `apps/web/src/components/MarkdownMessage.test.tsx`: cover semantics, safety, links, overflow, highlighting, and clipboard feedback.
- Modify `apps/web/src/components/ChatScreen.tsx`: replace plain user/assistant body text with `MarkdownMessage` while preserving bubble layout.
- Create `apps/web/src/components/ChatScreen.test.tsx`: prove both roles use Markdown and non-chat events remain plain text.
- Modify `apps/web/package.json` and `bun.lock`: declare the three runtime dependencies.

### Task 1: Install the Markdown Runtime Dependencies

**Files:**
- Modify: `apps/web/package.json`
- Modify: `bun.lock`

- [ ] **Step 1: Add direct web-app dependencies**

Run:

```powershell
bun add --cwd apps/web react-markdown remark-gfm prism-react-renderer
```

Expected: `apps/web/package.json` lists all three packages under `dependencies`, and `bun.lock` records their resolved versions.

- [ ] **Step 2: Verify package resolution through the web typecheck**

Run:

```powershell
bun x nx run '@animaOS-SWARM/web:typecheck' --skipNxCache
```

Expected: PASS before the packages are imported by application code.

- [ ] **Step 3: Commit the dependency boundary**

```powershell
git add -- apps/web/package.json bun.lock
git commit -m "build(web): add markdown rendering dependencies"
```

### Task 2: Build the Safe Markdown Presentation Component

**Files:**
- Create: `apps/web/src/components/MarkdownMessage.test.tsx`
- Create: `apps/web/src/components/MarkdownMessage.tsx`

- [ ] **Step 1: Write failing semantic and safety tests**

Create tests that render the wished-for API:

```tsx
render(<MarkdownMessage>{markdown}</MarkdownMessage>);
```

The tests must assert separately that:

1. headings, strong text, strikethrough, lists, task-list checkboxes, blockquotes, and a table render as semantic elements;
2. `<script data-testid="raw-script">alert(1)</script>` remains text and does not create a `script` element;
3. `[unsafe](javascript:alert(1))` has no executable `href`;
4. `https://example.com` opens with `target="_blank"` and `rel="noopener noreferrer"`, while `/settings` stays in the current tab;
5. tables expose a horizontal overflow wrapper and long inline code has wrapping styles.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```powershell
bun x nx test '@animaOS-SWARM/web' --run src/components/MarkdownMessage.test.tsx --skipNxCache
```

Expected: FAIL because `MarkdownMessage` does not exist.

- [ ] **Step 3: Implement minimal safe GFM rendering**

Create `MarkdownMessage.tsx` with this public contract:

```tsx
export function MarkdownMessage({ children }: { children: string }) {
  return (
    <div data-testid="markdown-message" className="min-w-0 break-words">
      <Markdown
        remarkPlugins={[remarkGfm]}
        urlTransform={defaultUrlTransform}
        components={markdownComponents}
      >
        {children}
      </Markdown>
    </div>
  );
}
```

Define scoped renderers for `h1`-`h3`, `p`, `ul`, `ol`, `li`, `blockquote`, `a`, `table`, `thead`, `th`, `td`, `hr`, `strong`, `del`, `pre`, and `code`. The custom `pre` renderer must return its child unchanged so the block-level `CodeBlock` owns the single `<pre>` element and never creates invalid nested `<pre>` markup. Do not add `rehype-raw`, `dangerouslySetInnerHTML`, math, diagrams, or rich-media handling.

For links, detect absolute HTTP/HTTPS URLs and add `target="_blank" rel="noopener noreferrer"`; leave relative and other safe URLs without a new-tab target. Preserve `defaultUrlTransform` so unsafe protocols become an empty URL.

For tables, the `table` renderer must return:

```tsx
<div data-markdown-overflow="table" className="my-3 max-w-full overflow-x-auto rounded-lg border border-line">
  <table className="w-full min-w-max border-collapse text-left text-xs">{children}</table>
</div>
```

- [ ] **Step 4: Run the focused test and verify GREEN**

Run the same focused test command. Expected: all semantic, safety, link, and overflow tests pass.

- [ ] **Step 5: Write failing highlighted-code and clipboard tests**

Add focused tests asserting that:

- inline code does not expose a Copy button;
- a `typescript` fence exposes its language and a Copy button;
- clicking Copy writes the exact source to `navigator.clipboard.writeText` and shows `Copied`;
- copied feedback returns to `Copy` after two seconds;
- a rejected or unavailable Clipboard API shows `Copy failed` without throwing; and
- an unknown language still renders the original source.

- [ ] **Step 6: Run the focused test and verify RED**

Expected: FAIL because the current `code` renderer has no highlighting/copy behavior.

- [ ] **Step 7: Implement the code block unit**

Add an internal `CodeBlock` component that:

- normalizes the fence language from `className="language-*"`;
- uses `Highlight` and `themes.vsDark` from `prism-react-renderer`;
- renders token spans through `getLineProps` and `getTokenProps`;
- falls back to plain code when highlighting cannot tokenize the supplied language;
- owns `idle | copied | failed` copy state;
- calls `navigator.clipboard.writeText(code)` only when the API exists;
- resets non-idle feedback to idle after 2,000 ms with effect cleanup; and
- exposes `data-markdown-overflow="code"` on its horizontally scrollable body.

The Markdown `code` renderer treats content as fenced when it has a `language-*` class or contains a newline. Inline code remains a styled `<code>` element.

- [ ] **Step 8: Run the focused test and verify GREEN**

Expected: every `MarkdownMessage` test passes with no unhandled promise rejection or React warning.

- [ ] **Step 9: Refactor while keeping the focused suite green**

Extract only small helpers that improve clarity, such as `languageFromClassName`, `isExternalHttpLink`, and `plainCode`. Re-run the focused suite after refactoring.

- [ ] **Step 10: Commit the reusable renderer**

```powershell
git add -- apps/web/src/components/MarkdownMessage.tsx apps/web/src/components/MarkdownMessage.test.tsx
git commit -m "feat(web): add safe rich markdown renderer"
```

### Task 3: Integrate Markdown into Both Chat Bubble Roles

**Files:**
- Create: `apps/web/src/components/ChatScreen.test.tsx`
- Modify: `apps/web/src/components/ChatScreen.tsx`

- [ ] **Step 1: Write failing chat integration tests**

Render `MessageList` with an `AgentDetail` containing User, Assistant, System, and Tool messages. Assert in separate tests that:

- user `**bold**` renders a `strong` element;
- assistant `## Heading` renders a level-two heading;
- System and Tool messages retain their literal Markdown markers in compact event pills; and
- timestamps and the conversation accessible label remain present.

- [ ] **Step 2: Run the focused integration test and verify RED**

Run:

```powershell
bun x nx test '@animaOS-SWARM/web' --run src/components/ChatScreen.test.tsx --skipNxCache
```

Expected: FAIL because user and assistant bubbles still render plain text.

- [ ] **Step 3: Delegate chat bodies to `MarkdownMessage`**

Import `MarkdownMessage` in `ChatScreen.tsx` and replace only this expression inside the User/Assistant bubble:

```tsx
{message.content.text}
```

with:

```tsx
<MarkdownMessage>{message.content.text}</MarkdownMessage>
```

Remove `whitespace-pre-wrap` from the bubble container because block whitespace is now owned by the Markdown element renderers. Do not change role filtering, event pills, alignment, visual bubble variants, timestamps, suggestions, sending state, or composer behavior.

- [ ] **Step 4: Run both focused suites and verify GREEN**

Run:

```powershell
bun x nx test '@animaOS-SWARM/web' --run src/components/MarkdownMessage.test.tsx src/components/ChatScreen.test.tsx --skipNxCache
```

Expected: both suites pass.

- [ ] **Step 5: Commit the chat integration**

```powershell
git add -- apps/web/src/components/ChatScreen.tsx apps/web/src/components/ChatScreen.test.tsx
git commit -m "feat(web): render markdown in chat bubbles"
```

### Task 4: Verify the Complete Web Surface

**Files:**
- Review: all files changed since the design commit

- [ ] **Step 1: Run the full uncached web gate**

Run:

```powershell
bun x nx run-many -t test typecheck build -p '@animaOS-SWARM/web' --skipNxCache
```

Expected: all web tests pass, TypeScript emits no errors, and the Vite production build exits successfully.

- [ ] **Step 2: Run repository hygiene checks**

Run:

```powershell
git diff --check
git status --short
```

Expected: no whitespace errors and only the intended plan/implementation files are changed.

- [ ] **Step 3: Inspect the live Markdown-heavy conversation**

Start the worktree web app on an unused local port if port 4200 is owned by the main checkout:

```powershell
bun x nx run '@animaOS-SWARM/web:dev' -- --port 4201 --strictPort
```

Open `http://localhost:4201/` in the in-app browser. Verify the existing Markdown-heavy assistant response renders semantic headings/lists, and use a controlled fixture or component test page only if persisted chat data is unavailable to the worktree host.

At desktop and narrow viewports, confirm bubbles remain contained, tables/code scroll inside the bubble, external links are safe, and the Copy button reports success.

- [ ] **Step 4: Review the acceptance criteria and branch diff**

Confirm every acceptance criterion in `docs/superpowers/specs/2026-08-31-web-chat-markdown-design.md` has direct test or browser evidence. Inspect `git diff main...HEAD` and ensure `anima.yaml` or unrelated workspace files are absent.

- [ ] **Step 5: Commit plan tracking updates if any**

If checkbox state was updated during execution, commit only the plan file:

```powershell
git add -- docs/superpowers/plans/2026-08-31-web-chat-markdown.md
git commit -m "docs(web): record markdown implementation plan"
```
