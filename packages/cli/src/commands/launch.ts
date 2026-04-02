import { Command } from "commander"
import { writeFileSync } from "node:fs"
import { join } from "node:path"
import yaml from "js-yaml"
import { EventBus } from "@animaOS-SWARM/core"
import type { AgentConfig, IEventBus, TaskResult } from "@animaOS-SWARM/core"
import { SwarmCoordinator } from "@animaOS-SWARM/swarm"
import { MemoryManager, createMemoryPlugin } from "@animaOS-SWARM/memory"
import { loadAgency, agencyExists } from "../agency/loader.js"
import { createAdapter } from "../agency/generator.js"
import { allTools } from "../tools.js"
import type { AgencyConfig, AgentDefinition } from "../agency/types.js"
import type { AgentProfile } from "@animaOS-SWARM/tui"

interface LaunchOptions {
	dir: string
	apiKey?: string
	tui: boolean
}

function agentDefToConfig(
	agent: AgentDefinition,
	defaultModel: string,
	memory?: MemoryManager,
): AgentConfig {
	return {
		name: agent.name,
		bio: agent.bio,
		lore: agent.lore,
		adjectives: agent.adjectives,
		topics: agent.topics,
		knowledge: agent.knowledge,
		style: agent.style,
		model: agent.model ?? defaultModel,
		system: agent.system,
		tools: allTools,
		plugins: memory ? [createMemoryPlugin(memory)] : [],
	}
}

function saveAgency(dir: string, agency: AgencyConfig) {
	writeFileSync(join(dir, "anima.yaml"), yaml.dump(agency, { lineWidth: 120, noRefs: true }))
}

export const launchCommand = new Command("launch")
	.description("Launch an agent swarm from an anima.yaml config")
	.argument("[task]", "The task to execute (omit to open interactive TUI session)")
	.option("-d, --dir <dir>", "Directory containing anima.yaml", ".")
	.option("--api-key <key>", "API key override")
	.option("--no-tui", "Disable TUI, use plain text output")
	.action(async (task: string | undefined, opts: LaunchOptions) => {
		if (!agencyExists(opts.dir)) {
			console.error(`Error: No anima.yaml found in "${opts.dir}". Run "animaos create" first.`)
			process.exit(1)
		}

		// Keep agency mutable so /agents edits propagate to the coordinator
		const agency = loadAgency(opts.dir)
		const adapter = createAdapter(agency.provider, opts.apiKey)
		const bus = new EventBus()

		// Memory — persists to <agency-dir>/.anima-memory.json
		const memory = new MemoryManager(join(opts.dir, ".anima-memory.json"))
		memory.load()

		/** Build a SwarmCoordinator from current agency state */
		function buildCoordinator() {
			return new SwarmCoordinator(
				{
					strategy: agency.strategy,
					manager: agentDefToConfig(agency.orchestrator, agency.model, memory),
					workers: agency.agents.map((a) => agentDefToConfig(a, agency.model, memory)),
				},
				adapter,
				bus,
			)
		}

		const interactive = !task

		if (opts.tui) {
			const { render } = await import("ink")
			const { default: React } = await import("react")
			const { App } = await import("@animaOS-SWARM/tui")

			// Build profiles from current agency (only AgentProfile fields)
			function toProfile(a: AgentDefinition, role: AgentProfile["role"]): AgentProfile {
				return {
					name: a.name, role,
					bio: a.bio, lore: a.lore,
					adjectives: a.adjectives, topics: a.topics,
					knowledge: a.knowledge, style: a.style,
					system: a.system,
				}
			}
			const agentProfiles: AgentProfile[] = [
				toProfile(agency.orchestrator, "orchestrator"),
				...agency.agents.map((a) => toProfile(a, "worker")),
			]

			/** Called when user edits an agent in the TUI — updates in-memory + writes yaml */
			function onSaveAgent(profile: AgentProfile) {
				if (profile.name === agency.orchestrator.name) {
					Object.assign(agency.orchestrator, profile)
				} else {
					const idx = agency.agents.findIndex((a) => a.name === profile.name)
					if (idx >= 0) Object.assign(agency.agents[idx], profile)
				}
				saveAgency(opts.dir, agency)
			}

			if (interactive) {
				// start a persistent coordinator once, reuse agents across tasks
				const coordinator = buildCoordinator()
				await coordinator.start()

				process.once("SIGINT", async () => {
					await coordinator.stop()
					process.exit(0)
				})

				const onTask = async (input: string): Promise<TaskResult> => {
					const result = await coordinator.dispatch(input)
					const text = result.status === "success"
						? (result.data as { text?: string })?.text ?? JSON.stringify(result.data, null, 2)
						: `Error: ${result.error}`
					writeFileSync(join(opts.dir, "anima-result.md"), `# Task\n\n${input}\n\n# Result\n\n${text}\n`)
					return result
				}

				const element = React.createElement(App, {
					eventBus: bus as IEventBus,
					strategy: agency.strategy,
					interactive: true,
					onTask,
					agentProfiles,
					onSaveAgent,
				})
				render(element)
				// Stay alive until Ctrl+C
			} else {
				// Single-shot TUI: spawn → run → unmount
				const coordinator = buildCoordinator()
				const element = React.createElement(App, {
					eventBus: bus as IEventBus,
					strategy: agency.strategy,
					task,
					agentProfiles,
					onSaveAgent,
				})
				const instance = render(element)

				const result = await coordinator.run(task!)
				await new Promise((resolve) => setTimeout(resolve, 500))
				instance.unmount()

				if (result.status === "error") process.exit(1)
			}
		} else {
			// Plain text mode
			bus.on("agent:spawned", (e) => {
				const d = e.data as { agentId: string; name: string }
				console.log(`  [agent] spawned: ${d.name} (${d.agentId})`)
			})
			bus.on("tool:before", (e) => {
				const d = e.data as { toolName: string }
				console.log(`  [tool] calling: ${d.toolName}`)
			})

			if (interactive) {
				// persistent coordinator for the whole session
				const coordinator = buildCoordinator()
				await coordinator.start()

				const { createInterface } = await import("node:readline")
				const rl = createInterface({ input: process.stdin, output: process.stdout })
				console.log(`${agency.name} — ${agency.strategy} strategy — ${agency.model}`)
				console.log('Type "exit" to quit.\n')

				rl.once("close", async () => { await coordinator.stop() })

				const prompt = () => {
					rl.question("task > ", async (input) => {
						const trimmed = input.trim()
						if (!trimmed || trimmed === "exit") {
							console.log("Bye.")
							rl.close()
							return
						}
						const result = await coordinator.dispatch(trimmed)
						console.log("\n--- Result ---")
						if (result.status === "success") {
							console.log((result.data as { text?: string })?.text ?? JSON.stringify(result.data))
						} else {
							console.error("Error:", result.error)
						}
						console.log(`Duration: ${result.durationMs}ms\n`)
						prompt()
					})
				}
				prompt()
			} else {
				// Single-shot plain text
				console.log(`Launching "${agency.name}" with strategy "${agency.strategy}" and model ${agency.model}...\n`)
				const result = await buildCoordinator().run(task!)
				console.log("\n--- Result ---")
				if (result.status === "success") {
					console.log((result.data as { text?: string })?.text ?? JSON.stringify(result.data))
				} else {
					console.error("Error:", result.error)
				}
				console.log(`\nDuration: ${result.durationMs}ms`)
			}
		}
	})
