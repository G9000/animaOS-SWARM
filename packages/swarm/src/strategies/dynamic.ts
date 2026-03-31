import { action, type TaskResult } from "@animaOS-SWARM/core"
import type { StrategyContext } from "../types.js"

export async function dynamicStrategy(ctx: StrategyContext): Promise<TaskResult> {
	const startTime = Date.now()

	// Spawn all workers in parallel — pool-aware spawnAgent returns existing agents instantly
	const agents = await Promise.all(
		ctx.workerConfigs.map(async (wc) => {
			const w = await ctx.spawnAgent(wc)
			return { id: w.id, name: wc.name, run: w.run }
		}),
	)

	const chatHistory: Array<{ speaker: string; content: string }> = []

	// Create choose_speaker action for manager
	const chooseSpeaker = action({
		name: "choose_speaker",
		description: "Choose which agent speaks next. Available agents: " +
			agents.map((a) => `"${a.name}"`).join(", ") +
			'. Set agent_name to "DONE" to end the conversation.',
		parameters: {
			type: "object",
			properties: {
				agent_name: { type: "string", description: "Name of the agent to speak next, or DONE to finish" },
				instruction: { type: "string", description: "What you want this agent to address" },
			},
			required: ["agent_name"],
		},
		handler: async (_runtime, _message, args) => {
			const agentName = args.agent_name as string
			const instruction = (args.instruction as string) ?? ""

			if (agentName === "DONE") {
				return { status: "success" as const, data: "DONE", durationMs: 0 }
			}

			const agent = agents.find((a) => a.name === agentName)
			if (!agent) {
				return {
					status: "error" as const,
					error: `Agent "${agentName}" not found. Available: ${agents.map((a) => a.name).join(", ")}`,
					durationMs: 0,
				}
			}

			// Build context for the selected agent
			const historyStr = chatHistory.length > 0
				? "\n\nConversation so far:\n" + chatHistory.map((h) => `[${h.speaker}]: ${h.content}`).join("\n")
				: ""

			const prompt = instruction + historyStr
			const result = await agent.run(prompt)

			const responseText = result.status === "success"
				? (result.data as { text: string })?.text ?? String(result.data)
				: `Error: ${result.error}`

			chatHistory.push({ speaker: agentName, content: responseText })

			return result
		},
	})

	// Spawn manager with choose_speaker tool
	const managerConfig = {
		...ctx.managerConfig,
		system: (ctx.managerConfig.system ?? "") +
			`\n\nYou are orchestrating a multi-agent conversation. Use choose_speaker to select which agent talks next.\n` +
			`Available agents: ${agents.map((a) => `"${a.name}"`).join(", ")}.\n` +
			`When you have enough information, call choose_speaker with agent_name="DONE" and provide your final synthesis.`,
		tools: [...(ctx.managerConfig.tools ?? []), chooseSpeaker],
	}

	const manager = await ctx.spawnAgent(managerConfig)
	const result = await manager.run(ctx.task)

	return { ...result, durationMs: Date.now() - startTime }
}
