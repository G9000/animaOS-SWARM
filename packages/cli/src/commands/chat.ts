import { Command } from "commander"
import { createInterface, type Interface } from "node:readline"
import { createCliDaemonClient, type CliDaemonClient } from "../client.js"

export interface ChatOptions {
	model: string
	name: string
	apiKey?: string
}

interface ChatDeps {
	client?: Pick<CliDaemonClient, "createAgent" | "runAgent">
	createReadline?: () => Pick<Interface, "question" | "close">
}

export async function executeChatCommand(
	opts: ChatOptions,
	deps: ChatDeps = {},
): Promise<void> {
	const client = deps.client ?? createCliDaemonClient()
	const agent = await client.createAgent({
		name: opts.name,
		model: opts.model,
		provider: "openai",
		system: "You are a helpful task agent. Be concise.",
	})

	console.log(`AnimaOS Kit - ${opts.name} (${opts.model})`)
	console.log('Type "exit" to quit.\n')

	const rl =
		deps.createReadline?.() ??
		createInterface({
			input: process.stdin,
			output: process.stdout,
		})

	await new Promise<void>((resolve) => {
		const prompt = () => {
			rl.question("you > ", async (input) => {
				const trimmed = input.trim()
				if (!trimmed || trimmed === "exit") {
					console.log("Bye.")
					rl.close()
					resolve()
					return
				}

				const result = await client.runAgent(agent.state.id, {
					text: trimmed,
				})

				if (result.result.status === "success") {
					const text =
						typeof result.result.data === "object" &&
						result.result.data !== null &&
						"text" in result.result.data
							? result.result.data.text
							: JSON.stringify(result.result.data)
					console.log(`\nagent > ${text}\n`)
				} else {
					console.log(`\n[error] ${result.result.error}\n`)
				}

				prompt()
			})
		}

		prompt()
	})
}

export const chatCommand = new Command("chat")
	.description("Interactive chat with an agent")
	.option("-m, --model <model>", "Model to use", "gpt-4o-mini")
	.option("-n, --name <name>", "Agent name", "task-agent")
	.option("--api-key <key>", "OpenAI API key (handled by the daemon if needed)")
	.action(executeChatCommand)
