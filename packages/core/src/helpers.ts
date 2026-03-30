import type { AgentConfig, Plugin, Action } from "./types/index.js"

/** Create an agent config */
export function agent(config: AgentConfig): AgentConfig {
	return config
}

/** Create a plugin */
export function plugin(config: Plugin): Plugin {
	return config
}

/** Create an action */
export function action(config: Action): Action {
	return config
}
