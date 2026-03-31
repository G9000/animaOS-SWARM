import type { Evaluator, EvaluatorResult, IAgentRuntime, Message, Content } from "@animaOS-SWARM/core"
import type { MemoryManager } from "./memory-manager.js"

/**
 * ObservationEvaluator — after each agent task, stores what happened as a memory.
 * Agents accumulate experience over time.
 *
 * Two memory types stored:
 * - task_result: what task was run and its outcome
 * - observation: a key takeaway from the result (if result is substantial)
 */
export class ObservationEvaluator implements Evaluator {
	readonly name = "observation"
	readonly description = "Stores task results and observations in memory"

	constructor(private manager: MemoryManager) {}

	async validate(_runtime: IAgentRuntime, _message: Message): Promise<boolean> {
		return true // Always run
	}

	async handler(runtime: IAgentRuntime, message: Message, response: Content): Promise<EvaluatorResult> {
		const task = message.content.text ?? ""
		const result = response.text ?? ""

		if (!task.trim() || !result.trim()) return {}

		// Store the task result
		this.manager.add({
			agentId: runtime.agentId,
			agentName: runtime.config.name,
			type: "task_result",
			content: `"${task.slice(0, 120)}" → ${result.slice(0, 300)}`,
			importance: 0.6,
			tags: [runtime.config.name, "task"],
		})

		// If the result is long enough, extract a shorter observation
		if (result.length > 100) {
			// Take the first sentence as the key takeaway
			const firstSentence = result.split(/[.!?]\s/)[0]?.trim()
			if (firstSentence && firstSentence.length > 20) {
				this.manager.add({
					agentId: runtime.agentId,
					agentName: runtime.config.name,
					type: "observation",
					content: firstSentence.slice(0, 200),
					importance: 0.5,
					tags: [runtime.config.name],
				})
			}
		}

		// Persist after every observation
		this.manager.save()

		return { metadata: { memorized: true } }
	}
}
