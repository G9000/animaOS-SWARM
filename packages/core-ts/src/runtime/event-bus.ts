import type { EventType, EventHandler, IEventBus, Event } from "../types/events.js"

export class EventBus implements IEventBus {
	private listeners = new Map<EventType, Array<EventHandler<unknown>>>()

	on<T = unknown>(type: EventType, handler: EventHandler<T>): () => void {
		const list = this.listeners.get(type) ?? []
		list.push(handler as EventHandler<unknown>)
		this.listeners.set(type, list)

		return () => {
			const current = this.listeners.get(type)
			if (current) {
				const idx = current.indexOf(handler as EventHandler<unknown>)
				if (idx >= 0) current.splice(idx, 1)
			}
		}
	}

	async emit<T = unknown>(type: EventType, data: T, agentId?: string): Promise<void> {
		const event: Event<T> = {
			type,
			agentId,
			timestamp: Date.now(),
			data,
		}

		const list = this.listeners.get(type)
		if (!list || list.length === 0) return

		for (const handler of list) {
			try {
				await (handler as EventHandler<T>)(event)
			} catch (err) {
				console.error(`[event-bus] Error in handler for ${type}:`, err)
			}
		}
	}

	clear(): void {
		this.listeners.clear()
	}
}
