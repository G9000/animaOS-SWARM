import type { TaskResult } from "@animaOS-SWARM/core"
import type { StrategyContext } from "../types.js"

export async function roundRobinStrategy(ctx: StrategyContext): Promise<TaskResult> {
	const startTime = Date.now()

	// Spawn all agents in parallel (manager + workers treated equally)
	// Pool-aware spawnAgent returns existing agents instantly on subsequent tasks
	const allConfigs = [ctx.managerConfig, ...ctx.workerConfigs]
	const agents = await Promise.all(
		allConfigs.map(async (config) => {
			const a = await ctx.spawnAgent(config)
			return { id: a.id, name: config.name, run: a.run }
		}),
	)

	const chatHistory: Array<{ speaker: string; content: string }> = []
	let lastResult: TaskResult | null = null

	const turns = ctx.maxTurns

	for (let turn = 0; turn < turns; turn++) {
		const agentIdx = turn % agents.length
		const agent = agents[agentIdx]

		// Build prompt with chat history
		const historyStr = chatHistory.length > 0
			? "\n\nConversation so far:\n" + chatHistory.map((h) => `[${h.speaker}]: ${h.content}`).join("\n")
			: ""

		const prompt = turn === 0
			? ctx.task
			: `Continue working on this task: ${ctx.task}\n\nIt's your turn to contribute.${historyStr}`

		const result = await agent.run(prompt)
		lastResult = result

		const responseText = result.status === "success"
			? (result.data as { text: string })?.text ?? String(result.data)
			: `Error: ${result.error}`

		chatHistory.push({ speaker: agent.name, content: responseText })
	}

	return {
		status: lastResult?.status ?? "error",
		data: {
			text: chatHistory.map((h) => `[${h.speaker}]: ${h.content}`).join("\n\n"),
			history: chatHistory,
		},
		durationMs: Date.now() - startTime,
	}
}
