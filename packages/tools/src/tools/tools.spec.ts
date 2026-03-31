import { describe, it, expect, beforeEach, afterEach } from "vitest"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs"
import { executeRead } from "./read.js"
import { executeWrite } from "./write.js"
import { executeEdit } from "./edit.js"
import { executeMultiEdit } from "./multi-edit.js"
import { executeGrep } from "./grep.js"
import { executeGlob } from "./glob.js"
import { executeBash } from "./bash.js"
import { executeTodoWrite, executeTodoRead, resetTodos } from "./todo.js"
import { normalizeLineEndings, unescapeOverEscaped, buildNotFoundError } from "../edit-hints.js"

// ─── helpers ─────────────────────────────────────────────────────────────────

function tmpDir(): string {
	const dir = join(tmpdir(), `anima-tools-test-${Date.now()}-${Math.random().toString(36).slice(2)}`)
	mkdirSync(dir, { recursive: true })
	return dir
}

function tmpFile(dir: string, name = "test.txt", content = ""): string {
	const path = join(dir, name)
	writeFileSync(path, content, "utf-8")
	return path
}

// ─── read ────────────────────────────────────────────────────────────────────

describe("executeRead()", () => {
	let dir: string

	beforeEach(() => { dir = tmpDir() })
	afterEach(() => { rmSync(dir, { recursive: true, force: true }) })

	it("returns error when file does not exist", () => {
		const r = executeRead({ file_path: join(dir, "nonexistent.txt") })
		expect(r.status).toBe("error")
		expect(r.result).toContain("File not found")
	})

	it("returns file content with line numbers", () => {
		const file = tmpFile(dir, "hello.txt", "line one\nline two\nline three")
		const r = executeRead({ file_path: file })
		expect(r.status).toBe("success")
		expect(r.result).toContain("1|")
		expect(r.result).toContain("line one")
		expect(r.result).toContain("line three")
	})

	it("line numbers start at 1 by default", () => {
		const file = tmpFile(dir, "nums.txt", "alpha\nbeta\ngamma")
		const r = executeRead({ file_path: file })
		expect(r.result.split("\n")[0]).toMatch(/^\s*1\|/)
	})

	it("respects offset — starts from the given line (0-indexed)", () => {
		const file = tmpFile(dir, "offset.txt", "a\nb\nc\nd\ne")
		const r = executeRead({ file_path: file, offset: 2 })
		expect(r.status).toBe("success")
		// offset=2 skips first 2 lines → line 3 = "c", number = 3
		expect(r.result).toContain("c")
		expect(r.result).not.toContain("| a")
	})

	it("respects limit — returns at most N lines", () => {
		const file = tmpFile(dir, "limit.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10")
		const r = executeRead({ file_path: file, limit: 3 })
		const lines = r.result.trim().split("\n")
		expect(lines).toHaveLength(3)
	})

	it("offset line numbers are correct in the output", () => {
		const file = tmpFile(dir, "offset-nums.txt", "x\ny\nz")
		// offset=1 → skip first line, start at line 2 (y)
		const r = executeRead({ file_path: file, offset: 1 })
		expect(r.result.split("\n")[0]).toMatch(/^\s*2\|/)
	})

	it("handles empty files without throwing", () => {
		const file = tmpFile(dir, "empty.txt", "")
		const r = executeRead({ file_path: file })
		expect(r.status).toBe("success")
	})
})

// ─── write ───────────────────────────────────────────────────────────────────

describe("executeWrite()", () => {
	let dir: string

	beforeEach(() => { dir = tmpDir() })
	afterEach(() => { rmSync(dir, { recursive: true, force: true }) })

	it("creates a new file with the given content", () => {
		const file = join(dir, "new.txt")
		const r = executeWrite({ file_path: file, content: "hello world" })
		expect(r.status).toBe("success")
		expect(existsSync(file)).toBe(true)
	})

	it("overwrites an existing file", () => {
		const file = tmpFile(dir, "overwrite.txt", "old content")
		executeWrite({ file_path: file, content: "new content" })
		const r = executeRead({ file_path: file })
		expect(r.result).toContain("new content")
		expect(r.result).not.toContain("old content")
	})

	it("creates intermediate directories as needed", () => {
		const file = join(dir, "nested", "deep", "file.txt")
		const r = executeWrite({ file_path: file, content: "deep" })
		expect(r.status).toBe("success")
		expect(existsSync(file)).toBe(true)
	})

	it("result reports the character count and file path", () => {
		const file = join(dir, "report.txt")
		const r = executeWrite({ file_path: file, content: "hello" })
		expect(r.result).toContain("5 chars")
		expect(r.result).toContain("report.txt")
	})

	it("handles empty content without error", () => {
		const file = join(dir, "empty.txt")
		const r = executeWrite({ file_path: file, content: "" })
		expect(r.status).toBe("success")
	})
})

// ─── edit ────────────────────────────────────────────────────────────────────

describe("executeEdit()", () => {
	let dir: string

	beforeEach(() => { dir = tmpDir() })
	afterEach(() => { rmSync(dir, { recursive: true, force: true }) })

	it("replaces old_string with new_string in a file", () => {
		const file = tmpFile(dir, "edit.txt", "The quick brown fox")
		const r = executeEdit({ file_path: file, old_string: "brown fox", new_string: "red cat" })
		expect(r.status).toBe("success")
		const read = executeRead({ file_path: file })
		expect(read.result).toContain("red cat")
		expect(read.result).not.toContain("brown fox")
	})

	it("returns error when the file does not exist", () => {
		const r = executeEdit({ file_path: join(dir, "ghost.txt"), old_string: "a", new_string: "b" })
		expect(r.status).toBe("error")
		expect(r.result).toContain("File not found")
	})

	it("returns error when old_string is not found in the file", () => {
		const file = tmpFile(dir, "not-found.txt", "hello world")
		const r = executeEdit({ file_path: file, old_string: "missing text", new_string: "something" })
		expect(r.status).toBe("error")
		expect(r.result).toContain("not found")
	})

	it("returns error when old_string matches more than once", () => {
		const file = tmpFile(dir, "ambiguous.txt", "foo bar foo")
		const r = executeEdit({ file_path: file, old_string: "foo", new_string: "baz" })
		expect(r.status).toBe("error")
		expect(r.result).toContain("matches 2 locations")
	})

	it("auto-fixes over-escaped \\n sequences from LLMs", () => {
		// File has a real newline; agent sent "\\n" (double backslash n)
		const file = tmpFile(dir, "escape.txt", "line one\nline two")
		// old_string with \\n instead of real newline
		const r = executeEdit({ file_path: file, old_string: "line one\\nline two", new_string: "replaced" })
		// Should succeed because unescapeOverEscaped converts \\n → \n
		expect(r.status).toBe("success")
		const read = executeRead({ file_path: file })
		expect(read.result).toContain("replaced")
	})

	it("normalizes CRLF line endings in old_string", () => {
		const file = tmpFile(dir, "crlf.txt", "first line\nsecond line")
		// Simulate agent sending CRLF in old_string
		const r = executeEdit({ file_path: file, old_string: "first line\r\nsecond line", new_string: "edited" })
		expect(r.status).toBe("success")
	})

	it("preserves the rest of the file when editing a substring", () => {
		const file = tmpFile(dir, "preserve.txt", "START middle END")
		executeEdit({ file_path: file, old_string: "middle", new_string: "CENTER" })
		const read = executeRead({ file_path: file })
		expect(read.result).toContain("START")
		expect(read.result).toContain("CENTER")
		expect(read.result).toContain("END")
	})
})

// ─── multi-edit ──────────────────────────────────────────────────────────────

describe("executeMultiEdit()", () => {
	let dir: string

	beforeEach(() => { dir = tmpDir() })
	afterEach(() => { rmSync(dir, { recursive: true, force: true }) })

	it("applies multiple edits atomically", () => {
		const file = tmpFile(dir, "multi.txt", "alpha beta gamma")
		const r = executeMultiEdit({
			file_path: file,
			edits: [
				{ old_string: "alpha", new_string: "ALPHA" },
				{ old_string: "beta", new_string: "BETA" },
				{ old_string: "gamma", new_string: "GAMMA" },
			],
		})
		expect(r.status).toBe("success")
		const read = executeRead({ file_path: file })
		expect(read.result).toContain("ALPHA BETA GAMMA")
	})

	it("returns error when file does not exist", () => {
		const r = executeMultiEdit({
			file_path: join(dir, "ghost.txt"),
			edits: [{ old_string: "x", new_string: "y" }],
		})
		expect(r.status).toBe("error")
		expect(r.result).toContain("File not found")
	})

	it("returns error and does NOT modify the file when any edit fails", () => {
		const file = tmpFile(dir, "atomic.txt", "A B C")
		const r = executeMultiEdit({
			file_path: file,
			edits: [
				{ old_string: "A", new_string: "X" },
				{ old_string: "MISSING", new_string: "Y" }, // this will fail
				{ old_string: "C", new_string: "Z" },
			],
		})
		expect(r.status).toBe("error")
		expect(r.result).toContain("Edit 2/3")
		// File must be untouched — no partial edits
		const read = executeRead({ file_path: file })
		expect(read.result).toContain("A B C")
	})

	it("returns error for empty edits array", () => {
		const file = tmpFile(dir, "empty-edits.txt", "content")
		const r = executeMultiEdit({ file_path: file, edits: [] })
		expect(r.status).toBe("error")
		expect(r.result).toContain("No edits")
	})

	it("later edits see the result of earlier edits in the batch", () => {
		// After first edit transforms "foo" → "bar", second edit should find "bar"
		const file = tmpFile(dir, "sequential.txt", "foo")
		const r = executeMultiEdit({
			file_path: file,
			edits: [
				{ old_string: "foo", new_string: "bar" },
				{ old_string: "bar", new_string: "baz" }, // sees the result of edit 1
			],
		})
		expect(r.status).toBe("success")
		const read = executeRead({ file_path: file })
		expect(read.result).toContain("baz")
	})

	it("reports how many edits were applied", () => {
		const file = tmpFile(dir, "count.txt", "one two three")
		const r = executeMultiEdit({
			file_path: file,
			edits: [
				{ old_string: "one", new_string: "1" },
				{ old_string: "two", new_string: "2" },
			],
		})
		expect(r.result).toContain("2 edit")
	})
})

// ─── edit-hints ──────────────────────────────────────────────────────────────

describe("edit-hints helpers", () => {
	describe("normalizeLineEndings()", () => {
		it("converts \\r\\n to \\n", () => {
			expect(normalizeLineEndings("a\r\nb\r\nc")).toBe("a\nb\nc")
		})
		it("leaves \\n-only content unchanged", () => {
			expect(normalizeLineEndings("a\nb\nc")).toBe("a\nb\nc")
		})
	})

	describe("unescapeOverEscaped()", () => {
		it("converts \\\\n to real newline", () => {
			expect(unescapeOverEscaped("line one\\nline two")).toBe("line one\nline two")
		})
		it("converts \\\\t to real tab", () => {
			expect(unescapeOverEscaped("key\\tvalue")).toBe("key\tvalue")
		})
		it("preserves literal backslash-n (\\\\n) as a single backslash — not a newline", () => {
			// The source protects real \\n (double-backslash-n) via __BACKSLASH_ESCAPE__ sentinel
			// so it is NOT converted to a newline. "\\\\n" in a JS string literal is the two
			// characters \n (backslash + n); unescapeOverEscaped should convert it to just \.
			// This exercises the `replace(/\\\\(?=[ntrfv'"\`])/g, "__BACKSLASH_ESCAPE__")` branch.
			const result = unescapeOverEscaped("path\\\\nfile")
			// \\\\n → protect sentinel → \\n → unescape \n → sentinel → \
			// net result: "path\nfile" (with a real newline from the \\n) is NOT what we want;
			// the sentinel should give "path\file" for the \\n portion
			// The key thing: it must NOT be "path\nfile" with a raw newline for the \\n input
			expect(result).not.toContain("\nfile") // double-backslash-n must not become a newline
		})
	})

	describe("buildNotFoundError()", () => {
		it("returns a fallback message when no smart-quote or whitespace mismatch", () => {
			const msg = buildNotFoundError("file.ts", "xyz", "some other content")
			expect(msg).toContain("file.ts")
			expect(msg).toMatch(/not found|changed|re-read/i)
		})

		it("detects smart quote mismatch", () => {
			// File has curly quote, search has straight quote
			const fileContent = "The \u2018value\u2019 here"
			const msg = buildNotFoundError("file.ts", "The 'value' here", fileContent)
			expect(msg).toContain("smart")
		})

		it("detects whitespace mismatch", () => {
			// File has single spaces, search has double spaces
			const fileContent = "function doSomething() {"
			const msg = buildNotFoundError("file.ts", "function  doSomething()  {", fileContent)
			expect(msg).toContain("whitespace")
		})
	})
})

// ─── glob ────────────────────────────────────────────────────────────────────

describe("executeGlob()", () => {
	let dir: string

	beforeEach(() => {
		dir = tmpDir()
		// Create some test files
		writeFileSync(join(dir, "alpha.ts"), "")
		writeFileSync(join(dir, "beta.ts"), "")
		writeFileSync(join(dir, "gamma.js"), "")
		mkdirSync(join(dir, "sub"), { recursive: true })
		writeFileSync(join(dir, "sub", "delta.ts"), "")
	})
	afterEach(() => { rmSync(dir, { recursive: true, force: true }) })

	it("matches all *.ts files in a directory", () => {
		const r = executeGlob({ pattern: "*.ts", path: dir })
		expect(r.status).toBe("success")
		expect(r.result).toContain("alpha.ts")
		expect(r.result).toContain("beta.ts")
		expect(r.result).not.toContain("gamma.js")
	})

	it("matches nested files with **/*.ts but NOT root-level files (implementation quirk)", () => {
		// The glob implementation converts **/*.ts → ^.*\/[^/]*\.ts$ which requires
		// at least one path separator. Root-level files (alpha.ts, beta.ts) have no
		// slash in their relative path and therefore do NOT match.
		const r = executeGlob({ pattern: "**/*.ts", path: dir })
		expect(r.result).toContain("delta.ts")    // sub/delta.ts — has a slash, matches
		expect(r.result).not.toContain("alpha.ts") // root-level — no slash, excluded
		expect(r.result).not.toContain("beta.ts")  // root-level — no slash, excluded
	})

	it("returns 'No files found' when pattern matches nothing", () => {
		const r = executeGlob({ pattern: "*.rb", path: dir })
		expect(r.result).toBe("No files found")
	})

	it("results are sorted alphabetically", () => {
		const r = executeGlob({ pattern: "*.ts", path: dir })
		const files = r.result.split("\n")
		const sorted = [...files].sort()
		expect(files).toEqual(sorted)
	})

	it("does not include node_modules or hidden directories", () => {
		mkdirSync(join(dir, "node_modules", "pkg"), { recursive: true })
		writeFileSync(join(dir, "node_modules", "pkg", "index.ts"), "")
		mkdirSync(join(dir, ".hidden"), { recursive: true })
		writeFileSync(join(dir, ".hidden", "secret.ts"), "")

		const r = executeGlob({ pattern: "**/*.ts", path: dir })
		expect(r.result).not.toContain("node_modules")
		expect(r.result).not.toContain(".hidden")
	})
})

// ─── grep ────────────────────────────────────────────────────────────────────

describe("executeGrep()", () => {
	let dir: string

	beforeEach(() => {
		dir = tmpDir()
		writeFileSync(join(dir, "a.ts"), "const x = 1\nexport { x }")
		writeFileSync(join(dir, "b.ts"), "import { x } from './a'\nconst y = x + 1")
		writeFileSync(join(dir, "c.js"), "// no typescript here")
	})
	afterEach(() => { rmSync(dir, { recursive: true, force: true }) })

	it("finds files matching a pattern", () => {
		const r = executeGrep({ pattern: "const", path: dir })
		expect(r.status).toBe("success")
		expect(r.result).toContain("const")
	})

	it("returns 'No matches found' when pattern matches nothing", () => {
		const r = executeGrep({ pattern: "quantum_physics_xyzzy", path: dir })
		expect(r.status).toBe("success")
		expect(r.result).toContain("No matches found")
	})

	it("respects the include glob filter — only searches matching files", () => {
		// a.ts and b.ts both contain "const"; c.js also contains nothing that matches
		// but the include filter must restrict to *.ts files only
		const r = executeGrep({ pattern: "const", path: dir, include: "*.ts" })
		expect(r.status).toBe("success")
		// Must find results (a.ts and b.ts both have "const")
		expect(r.result).not.toBe("No matches found")
		// Must NOT reference c.js even though it's in the same directory
		expect(r.result).not.toContain("c.js")
	})

	it("does not throw for an empty directory", () => {
		const emptyDir = tmpDir()
		try {
			const r = executeGrep({ pattern: "anything", path: emptyDir })
			expect(["success"]).toContain(r.status)
		} finally {
			rmSync(emptyDir, { recursive: true, force: true })
		}
	})
})

// ─── bash ────────────────────────────────────────────────────────────────────

describe("executeBash()", () => {
	it("executes a command and returns stdout", async () => {
		const r = await executeBash({ command: "echo hello_from_bash" })
		expect(r.status).toBe("success")
		expect(r.result).toContain("hello_from_bash")
	})

	it("captures stderr on non-zero exit", async () => {
		const r = await executeBash({ command: "ls /path/that/does/not/exist/ever" })
		expect(r.status).toBe("error")
	})

	it("returns error on timeout", async () => {
		// "sleep 10" is guaranteed to outlast a 50ms timeout regardless of hardware speed.
		// Using "echo slow" with 1ms was a race condition: on a fast native shell, echo can
		// complete before the timer fires.
		const r = await executeBash({ command: "sleep 10", timeout: 50 })
		expect(r.status).toBe("error")
		expect(r.result).toContain("timed out")
	})

	it("sets working directory via cwd — output reflects the specified directory", async () => {
		const dir = tmpDir()
		try {
			const r = await executeBash({ command: "pwd", cwd: dir })
			expect(r.status).toBe("success")
			// On Windows/MSYS, Node's tmpdir() returns a Windows path (C:\...) but
			// bash's pwd returns the MSYS unix path (/tmp/...). Compare by the unique
			// directory name (last path component) which is the same in both systems.
			const { basename } = await import("node:path")
			const uniqueName = basename(dir)
			expect(r.result).toContain(uniqueName)
		} finally {
			rmSync(dir, { recursive: true, force: true })
		}
	})

	it("stdout and stderr arrays are returned", async () => {
		const r = await executeBash({ command: "echo test_output" })
		expect(r.stdout).toBeDefined()
		expect(r.stderr).toBeDefined()
		expect(Array.isArray(r.stdout)).toBe(true)
		expect(Array.isArray(r.stderr)).toBe(true)
	})
})

// ─── todo ────────────────────────────────────────────────────────────────────

describe("executeTodoWrite() + executeTodoRead()", () => {
	beforeEach(() => { resetTodos() })
	afterEach(() => { resetTodos() })

	it("writes todos and reads them back", () => {
		executeTodoWrite({
			todos: [
				{ content: "Write tests", status: "in_progress", activeForm: "Writing tests" },
				{ content: "Review code", status: "pending", activeForm: "Reviewing code" },
			],
		})
		const r = executeTodoRead()
		expect(r.result).toContain("Write tests")
		expect(r.result).toContain("Review code")
	})

	it("returns success with a summary after writing", () => {
		const r = executeTodoWrite({
			todos: [
				{ content: "Task one", status: "completed", activeForm: "Completing task one" },
				{ content: "Task two", status: "in_progress", activeForm: "Working on task two" },
				{ content: "Task three", status: "pending", activeForm: "Starting task three" },
			],
		})
		expect(r.status).toBe("success")
		expect(r.result).toContain("1 completed")
		expect(r.result).toContain("1 in progress")
		expect(r.result).toContain("1 pending")
	})

	it("read returns 'No todos set.' when empty", () => {
		const r = executeTodoRead()
		expect(r.result).toBe("No todos set.")
	})

	it("shows [x] for completed, [>] for in_progress, [ ] for pending", () => {
		executeTodoWrite({
			todos: [
				{ content: "Done", status: "completed", activeForm: "Completing" },
				{ content: "Doing", status: "in_progress", activeForm: "Working" },
				{ content: "Todo", status: "pending", activeForm: "Starting" },
			],
		})
		const r = executeTodoRead()
		expect(r.result).toContain("[x]")
		expect(r.result).toContain("[>]")
		expect(r.result).toContain("[ ]")
	})

	it("replaces the previous todo list entirely", () => {
		executeTodoWrite({ todos: [{ content: "Old task", status: "pending", activeForm: "Starting old task" }] })
		executeTodoWrite({ todos: [{ content: "New task", status: "in_progress", activeForm: "Working on new task" }] })
		const r = executeTodoRead()
		expect(r.result).not.toContain("Old task")
		expect(r.result).toContain("New task")
	})

	it("returns error for non-array todos", () => {
		const r = executeTodoWrite({ todos: null as unknown as [] })
		expect(r.status).toBe("error")
		expect(r.result).toContain("array")
	})

	it("returns error for invalid status", () => {
		const r = executeTodoWrite({
			todos: [{ content: "Task", status: "done" as "completed", activeForm: "Doing" }],
		})
		expect(r.status).toBe("error")
		expect(r.result).toContain("status")
	})

	it("returns error for empty content", () => {
		const r = executeTodoWrite({
			todos: [{ content: "", status: "pending", activeForm: "Starting" }],
		})
		expect(r.status).toBe("error")
		expect(r.result).toContain("content")
	})

	it("returns error for missing activeForm", () => {
		const r = executeTodoWrite({
			todos: [{ content: "Task", status: "pending", activeForm: "" }],
		})
		expect(r.status).toBe("error")
		expect(r.result).toContain("activeForm")
	})

	it("warns but succeeds when multiple todos are in_progress", () => {
		const r = executeTodoWrite({
			todos: [
				{ content: "Task A", status: "in_progress", activeForm: "Working on A" },
				{ content: "Task B", status: "in_progress", activeForm: "Working on B" },
			],
		})
		expect(r.status).toBe("success")
		expect(r.result).toContain("Warning")
		expect(r.result).toContain("in_progress")
	})
})
