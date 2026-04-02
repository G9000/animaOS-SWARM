import { Command } from "commander"
import { createCliDaemonClient, type CliDaemonClient } from "../client.js"

export interface RunOptions {
	model: string
	provider: string
	name: string
	strategy?: "supervisor" | "dynamic" | "round-robin"
	apiKey?: string
	tui: boolean
}

export async function executeRunCommand(
	task: string,
	opts: RunOptions,
	client: CliDaemonClient = createCliDaemonClient(),
): Promise<void> {
	if (opts.tui) {
		console.log("TUI mode is not available for daemon-backed runs yet. Falling back to plain text.\n")
	}

	const execution = await client.runTask(task, {
		model: opts.model,
		provider: opts.provider,
		name: opts.name,
		strategy: opts.strategy,
	})

	if (execution.mode === "swarm") {
		console.log(`Swarm running with strategy "${opts.strategy}" and model ${opts.model}...\n`)
	} else {
		console.log(`Agent "${opts.name}" running with ${opts.model}...\n`)
	}

	console.log("--- Result ---")
	if (execution.result.status === "success") {
		const output =
			typeof execution.result.data === "object" &&
			execution.result.data !== null &&
			"text" in execution.result.data
				? execution.result.data.text
				: JSON.stringify(execution.result.data)
		console.log(output)
	} else {
		console.error("Error:", execution.result.error)
		process.exitCode = 1
	}

	const duration = execution.result.durationMs ?? 0
	if (execution.mode === "swarm") {
		console.log(`\nDuration: ${duration}ms | Tokens: ${execution.swarm.tokenUsage.totalTokens}`)
	} else {
		console.log(`\nDuration: ${duration}ms | Tokens: ${execution.agent.state.tokenUsage.totalTokens}`)
	}
}

export const runCommand = new Command("run")
	.description("Run an agent or swarm with a task")
	.argument("<task>", "The task to execute")
	.option("-m, --model <model>", "Model to use", "gpt-4o-mini")
	.option("-p, --provider <provider>", "Provider: openai, anthropic, ollama", "openai")
	.option("-n, --name <name>", "Agent name (single-agent mode)", "task-agent")
	.option("-s, --strategy <strategy>", "Swarm strategy: supervisor, dynamic, round-robin")
	.option("--api-key <key>", "API key override")
	.option("--no-tui", "Disable TUI, use plain text output")
	.action(executeRunCommand)
