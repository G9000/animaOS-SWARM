export type EventType =
	| "agent:spawned"
	| "agent:started"
	| "agent:completed"
	| "agent:failed"
	| "agent:terminated"
	| "agent:message"
	| "task:started"
	| "task:completed"
	| "task:failed"
	| "tool:before"
	| "tool:after"
	| "agent:tokens"
	| "swarm:created"
	| "swarm:message"
	| "swarm:completed"
	| "swarm:stopped"

export interface Event<T = unknown> {
	id: string
	type: EventType
	agentId?: string
	timestampMs: number
	data: T
}

export type EventHandler<T = unknown> = (event: Event<T>) => void | Promise<void>

export interface IEventBus {
	on<T = unknown>(type: EventType, handler: EventHandler<T>): () => void
	emit<T = unknown>(type: EventType, data: T, agentId?: string): Promise<void>
	clear(): void
}
