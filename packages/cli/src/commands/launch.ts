import { Command } from "commander"
import { EventBus } from "@animaOS-SWARM/core"
import type { AgentConfig, IEventBus } from "@animaOS-SWARM/core"
import { SwarmCoordinator } from "@animaOS-SWARM/swarm"
import { loadAgency, agencyExists } from "../agency/loader.js"
import { createAdapter } from "../agency/generator.js"
import { allTools } from "../tools.js"

interface LaunchOptions {
	dir: string
	apiKey?: string
	tui: boolean
}

export const launchCommand = new Command("launch")
	.description("Launch an agent swarm from an anima.yaml config")
	.argument("<task>", "The task to execute")
	.option("-d, --dir <dir>", "Directory containing anima.yaml", ".")
	.option("--api-key <key>", "API key override")
	.option("--no-tui", "Disable TUI, use plain text output")
	.action(async (task: string, opts: LaunchOptions) => {
		if (!agencyExists(opts.dir)) {
			console.error(`Error: No anima.yaml found in "${opts.dir}". Run "anima create" first.`)
			process.exit(1)
		}

		const agency = loadAgency(opts.dir)
		const adapter = createAdapter(agency.provider, opts.apiKey)
		const bus = new EventBus()

		const managerConfig: AgentConfig = {
			name: agency.orchestrator.name,
			bio: agency.orchestrator.bio,
			model: agency.orchestrator.model ?? agency.model,
			system: agency.orchestrator.system,
			tools: allTools,
		}

		const workerConfigs: AgentConfig[] = agency.agents.map((agent) => ({
			name: agent.name,
			bio: agent.bio,
			model: agent.model ?? agency.model,
			system: agent.system,
			tools: allTools,
		}))

		const coordinator = new SwarmCoordinator(
			{
				strategy: agency.strategy,
				manager: managerConfig,
				workers: workerConfigs,
			},
			adapter,
			bus,
		)

		if (opts.tui) {
			const { render } = await import("ink")
			const { default: React } = await import("react")
			const { App } = await import("@animaOS-SWARM/tui")

			const element = React.createElement(App, {
				eventBus: bus as IEventBus,
				strategy: agency.strategy,
				task,
			})
			const instance = render(element)

			const result = await coordinator.run(task)
			await new Promise((resolve) => setTimeout(resolve, 500))
			instance.unmount()

			if (result.status === "error") {
				process.exit(1)
			}
		} else {
			bus.on("agent:spawned", (e) => {
				const d = e.data as { agentId: string; name: string }
				console.log(`  [agent] spawned: ${d.name} (${d.agentId})`)
			})
			bus.on("tool:before", (e) => {
				const d = e.data as { toolName: string }
				console.log(`  [tool] calling: ${d.toolName}`)
			})

			console.log(`Launching "${agency.name}" with strategy "${agency.strategy}" and model ${agency.model}...\n`)
			const result = await coordinator.run(task)

			console.log("\n--- Result ---")
			if (result.status === "success") {
				const data = result.data as { text?: string } | undefined
				console.log(data?.text ?? JSON.stringify(result.data))
			} else {
				console.error("Error:", result.error)
			}
			console.log(`\nDuration: ${result.durationMs}ms`)
		}
	})
