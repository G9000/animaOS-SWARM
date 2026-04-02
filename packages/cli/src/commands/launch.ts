import { Command } from "commander"
import { createInterface } from "node:readline"
import { agencyExists, loadAgency } from "../agency/loader.js"
import { createCliDaemonClient, type CliDaemonClient } from "../client.js"

interface LaunchOptions {
	dir: string
	apiKey?: string
	tui: boolean
}

export async function executeLaunchCommand(
	task: string | undefined,
	opts: LaunchOptions,
	client: CliDaemonClient = createCliDaemonClient(),
): Promise<void> {
	if (!agencyExists(opts.dir)) {
		console.error(`Error: No anima.yaml found in "${opts.dir}". Run "animaos create" first.`)
		process.exitCode = 1
		return
	}

	if (opts.tui) {
		console.log("TUI mode is not available for daemon-backed launch yet. Falling back to plain text.\n")
	}

	const agency = loadAgency(opts.dir)
	const swarm = await client.createAgencySwarm(agency)
	const interactive = !task

	if (!interactive) {
		console.log(`Launching "${agency.name}" with strategy "${agency.strategy}" and model ${agency.model}...\n`)
		const result = await client.runSwarm(swarm.id, { text: task })
		renderLaunchResult(result.result)
		return
	}

	const rl = createInterface({ input: process.stdin, output: process.stdout })
	console.log(`${agency.name} - ${agency.strategy} strategy - ${agency.model}`)
	console.log('Type "exit" to quit.\n')

	await new Promise<void>((resolve) => {
		const prompt = () => {
			rl.question("task > ", async (input) => {
				const trimmed = input.trim()
				if (!trimmed || trimmed === "exit") {
					console.log("Bye.")
					rl.close()
					resolve()
					return
				}

				const result = await client.runSwarm(swarm.id, { text: trimmed })
				renderLaunchResult(result.result)
				prompt()
			})
		}

		prompt()
	})
}

function renderLaunchResult(result: Awaited<ReturnType<CliDaemonClient["runSwarm"]>>["result"]) {
	console.log("--- Result ---")
	if (result.status === "success") {
		const output =
			typeof result.data === "object" && result.data !== null && "text" in result.data
				? result.data.text
				: JSON.stringify(result.data)
		console.log(output)
	} else {
		console.error("Error:", result.error)
		process.exitCode = 1
	}
	console.log(`\nDuration: ${result.durationMs}ms\n`)
}

export const launchCommand = new Command("launch")
	.description("Launch an agent swarm from an anima.yaml config")
	.argument("[task]", "The task to execute (omit to open interactive session)")
	.option("-d, --dir <dir>", "Directory containing anima.yaml", ".")
	.option("--api-key <key>", "API key override")
	.option("--no-tui", "Disable TUI, use plain text output")
	.action(executeLaunchCommand)
