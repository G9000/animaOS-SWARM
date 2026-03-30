export { SwarmCoordinator } from "./coordinator.js"
export { MessageBus } from "./message-bus.js"
export { supervisorStrategy, dynamicStrategy, roundRobinStrategy } from "./strategies/index.js"
export type {
	SwarmConfig,
	SwarmStrategy,
	SwarmState,
	AgentMessage,
	StrategyContext,
	IMessageBus,
} from "./types.js"

import type { IModelAdapter, IEventBus } from "@animaOS-SWARM/core"
import { SwarmCoordinator } from "./coordinator.js"
import type { SwarmConfig } from "./types.js"

/** Create a swarm coordinator */
export function swarm(config: SwarmConfig, modelAdapter: IModelAdapter, eventBus?: IEventBus): SwarmCoordinator {
	return new SwarmCoordinator(config, modelAdapter, eventBus)
}
