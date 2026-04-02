import type { Content, TaskResult } from "@animaOS-SWARM/core"
import type { SwarmConfig, SwarmState } from "@animaOS-SWARM/swarm"

import type { DaemonClient, DaemonEvent } from "./client.js"

export interface SwarmRunResponse<T = { text: string; [key: string]: unknown }> {
	swarm: SwarmState
	result: TaskResult<T>
}

export interface SwarmEventPayload<T = unknown> {
	swarmId: string
	state: SwarmState
	result: TaskResult<T> | null
}

export function swarm<T extends SwarmConfig>(config: T): T {
	return config
}

export class SwarmsClient {
	constructor(private readonly client: DaemonClient) {}

	async create(config: SwarmConfig): Promise<SwarmState> {
		const response = await this.client.requestJson<{ swarm: SwarmState }>("/api/swarms", {
			method: "POST",
			body: config,
		})

		return response.swarm
	}

	async get(swarmId: string): Promise<SwarmState> {
		const response = await this.client.requestJson<{ swarm: SwarmState }>(
			`/api/swarms/${swarmId}`,
		)
		return response.swarm
	}

	async run(
		swarmId: string,
		input: Content,
	): Promise<SwarmRunResponse<{ text: string; [key: string]: unknown }>> {
		return this.client.requestJson<SwarmRunResponse<{ text: string; [key: string]: unknown }>>(
			`/api/swarms/${swarmId}/run`,
			{
				method: "POST",
				body: input,
			},
		)
	}

	subscribe<T = unknown>(swarmId: string): AsyncGenerator<DaemonEvent<SwarmEventPayload<T>>> {
		return this.client.subscribe<SwarmEventPayload<T>>(`/api/swarms/${swarmId}/events`)
	}
}
