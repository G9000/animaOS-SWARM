import { randomUUID } from "node:crypto"
import type { Content } from "@animaOS-SWARM/core"
import type { AgentMessage, IMessageBus } from "./types.js"

export class MessageBus implements IMessageBus {
	private inboxes = new Map<string, AgentMessage[]>()
	private allMessages: AgentMessage[] = []

	registerAgent(agentId: string): void {
		if (!this.inboxes.has(agentId)) {
			this.inboxes.set(agentId, [])
		}
	}

	unregisterAgent(agentId: string): void {
		this.inboxes.delete(agentId)
	}

	send(from: string, to: string, content: Content): void {
		const msg: AgentMessage = {
			id: randomUUID(),
			from,
			to,
			content,
			timestamp: Date.now(),
		}
		this.allMessages.push(msg)

		const inbox = this.inboxes.get(to)
		if (inbox) {
			inbox.push(msg)
		}
	}

	broadcast(from: string, content: Content): void {
		const msg: AgentMessage = {
			id: randomUUID(),
			from,
			to: "broadcast",
			content,
			timestamp: Date.now(),
		}
		this.allMessages.push(msg)

		for (const [agentId, inbox] of this.inboxes) {
			if (agentId !== from) {
				inbox.push(msg)
			}
		}
	}

	getMessages(agentId: string): AgentMessage[] {
		return this.inboxes.get(agentId) ?? []
	}

	getAllMessages(): AgentMessage[] {
		return [...this.allMessages]
	}

	clear(): void {
		this.inboxes.clear()
		this.allMessages = []
	}
}
