import { Command } from "commander"
import pc from "picocolors"
import { loadAgency, agencyExists } from "../agency/loader.js"
import type { AgentDefinition } from "../agency/types.js"

function renderAgent(agent: AgentDefinition, isOrchestrator: boolean) {
	const prefix = isOrchestrator ? pc.yellow("★") : pc.dim("•")
	const role = isOrchestrator ? pc.dim(" (orchestrator)") : pc.dim(" (worker)")
	console.log(`\n  ${prefix} ${pc.cyan(pc.bold(agent.name))}${role}`)
	console.log(`    ${pc.dim("bio:")} ${agent.bio}`)
	if (agent.adjectives && agent.adjectives.length > 0) {
		console.log(`    ${pc.dim("personality:")} ${agent.adjectives.join(", ")}`)
	}
	if (agent.topics && agent.topics.length > 0) {
		console.log(`    ${pc.dim("topics:")} ${agent.topics.join(", ")}`)
	}
}

function renderAgentFull(agent: AgentDefinition, isOrchestrator: boolean) {
	const role = isOrchestrator ? "orchestrator" : "worker"
	console.log(pc.bold(pc.cyan(`\n  ${agent.name}`)) + pc.dim(` (${role})`))
	console.log()
	console.log(`  ${pc.bold("Bio")}`)
	console.log(`    ${agent.bio}`)
	if (agent.lore) {
		console.log()
		console.log(`  ${pc.bold("Backstory")}`)
		console.log(`    ${agent.lore}`)
	}
	if (agent.adjectives && agent.adjectives.length > 0) {
		console.log()
		console.log(`  ${pc.bold("Personality")}`)
		console.log(`    ${agent.adjectives.join(", ")}`)
	}
	if (agent.topics && agent.topics.length > 0) {
		console.log()
		console.log(`  ${pc.bold("Expertise")}`)
		console.log(`    ${agent.topics.join(", ")}`)
	}
	if (agent.knowledge && agent.knowledge.length > 0) {
		console.log()
		console.log(`  ${pc.bold("Knowledge")}`)
		for (const k of agent.knowledge) {
			console.log(`    ${pc.dim("-")} ${k}`)
		}
	}
	if (agent.style) {
		console.log()
		console.log(`  ${pc.bold("Communication Style")}`)
		console.log(`    ${agent.style}`)
	}
	console.log()
	console.log(`  ${pc.bold("System Prompt")}`)
	console.log(`    ${agent.system}`)
}

const listCommand = new Command("list")
	.description("List agents in an agency")
	.option("-d, --dir <dir>", "Directory containing anima.yaml", ".")
	.action((opts: { dir: string }) => {
		if (!agencyExists(opts.dir)) {
			console.error(`Error: No anima.yaml found in "${opts.dir}". Run "animaos create" first.`)
			process.exit(1)
		}
		const agency = loadAgency(opts.dir)
		const total = 1 + agency.agents.length

		console.log()
		console.log(pc.bold(`  ${agency.name}`))
		console.log(pc.dim(`  ${agency.description}`))
		console.log(pc.dim(`  strategy: ${agency.strategy} · model: ${agency.model} · ${total} agent${total !== 1 ? "s" : ""}`))

		renderAgent(agency.orchestrator, true)
		for (const agent of agency.agents) {
			renderAgent(agent, false)
		}
		console.log()
	})

const showCommand = new Command("show")
	.description("Show full details of an agent")
	.argument("<name>", "Agent name")
	.option("-d, --dir <dir>", "Directory containing anima.yaml", ".")
	.action((name: string, opts: { dir: string }) => {
		if (!agencyExists(opts.dir)) {
			console.error(`Error: No anima.yaml found in "${opts.dir}". Run "animaos create" first.`)
			process.exit(1)
		}
		const agency = loadAgency(opts.dir)

		const allAgents = [
			{ agent: agency.orchestrator, isOrchestrator: true },
			...agency.agents.map((a) => ({ agent: a, isOrchestrator: false })),
		]

		const match = allAgents.find(({ agent }) => agent.name === name)
		if (!match) {
			const names = allAgents.map(({ agent }) => agent.name).join(", ")
			console.error(`Error: Agent "${name}" not found. Available: ${names}`)
			process.exit(1)
		}

		console.log()
		renderAgentFull(match.agent, match.isOrchestrator)
	})

export const agentsCommand = new Command("agents")
	.description("Manage agents in an agency")
	.addCommand(listCommand)
	.addCommand(showCommand)
