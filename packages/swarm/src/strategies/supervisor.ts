import { action, type TaskResult } from "@animaOS-SWARM/core"
import type { StrategyContext } from "../types.js"

export async function supervisorStrategy(ctx: StrategyContext): Promise<TaskResult> {
	const startTime = Date.now()

	// Spawn all workers in parallel — pool-aware spawnAgent returns existing agents instantly
	const workers = await Promise.all(
		ctx.workerConfigs.map(async (wc) => {
			const w = await ctx.spawnAgent(wc)
			return { id: w.id, name: wc.name, run: w.run }
		}),
	)

	// Collect worker results
	const workerResults = new Map<string, TaskResult>()

	// Create delegate action for the manager
	const delegateTask = action({
		name: "delegate_task",
		description: "Delegate a subtask to a worker agent. Available workers: " +
			workers.map((w) => `"${w.name}"`).join(", "),
		parametersSchema: {
			type: "object",
			properties: {
				worker_name: { type: "string", description: "Name of the worker to delegate to" },
				task: { type: "string", description: "The subtask to delegate" },
			},
			required: ["worker_name", "task"],
		},
		handler: async (_runtime, _message, args) => {
			const workerName = args.worker_name as string
			const task = args.task as string
			const worker = workers.find((w) => w.name === workerName)

			if (!worker) {
				return {
					status: "error" as const,
					error: `Worker "${workerName}" not found. Available: ${workers.map((w) => w.name).join(", ")}`,
					durationMs: 0,
				}
			}

			const result = await worker.run(task)
			workerResults.set(workerName, result)
			return result
		},
	})

	// Spawn manager with delegate tool
	const managerConfig = {
		...ctx.managerConfig,
		system: (ctx.managerConfig.system ?? "") +
			`\n\nYou are a supervisor agent. You have worker agents available to delegate tasks to.\n` +
			`Available workers: ${workers.map((w) => `"${w.name}"`).join(", ")}.\n` +
			`Use the delegate_task tool to assign subtasks. Synthesize the results into a final answer.`,
		tools: [...(ctx.managerConfig.tools ?? []), delegateTask],
	}

	const manager = await ctx.spawnAgent(managerConfig)
	const result = await manager.run(ctx.task)

	return { ...result, durationMs: Date.now() - startTime }
}
