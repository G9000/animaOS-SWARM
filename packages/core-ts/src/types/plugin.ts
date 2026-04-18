import type { IAgentRuntime } from "./agent.js"
import type { Action, Evaluator, Provider } from "./components.js"

/**
 * Plugin — bundles actions, providers, and evaluators together.
 * Plugins are the primary way to extend agent capabilities.
 */
export interface Plugin {
	name: string
	description: string
	actions?: Action[]
	providers?: Provider[]
	evaluators?: Evaluator[]

	/** Called when the plugin is registered with a runtime */
	init?: (runtime: IAgentRuntime) => Promise<void>

	/** Called when the runtime shuts down */
	cleanup?: (runtime: IAgentRuntime) => Promise<void>
}
