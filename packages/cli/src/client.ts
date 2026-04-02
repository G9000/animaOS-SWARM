import type { AgencyConfig, AgentDefinition } from "./agency/types.js"

const DEFAULT_DAEMON_URL = "http://127.0.0.1:3000"

export type SwarmStrategy = "supervisor" | "dynamic" | "round-robin"

export interface Content {
	text: string
	attachments?: unknown[]
	metadata?: Record<string, unknown>
}

export interface TaskResult<T = Content> {
	status: "success" | "error"
	data?: T
	error?: string
	durationMs: number
}

export interface CliAgentConfig {
	name: string
	model: string
	provider?: string
	system?: string
	bio?: string
	lore?: string
	knowledge?: string[]
	topics?: string[]
	adjectives?: string[]
	style?: string
}

export interface TokenUsage {
	promptTokens: number
	completionTokens: number
	totalTokens: number
}

export interface AgentState {
	id: string
	name: string
	status: "idle" | "running" | "completed" | "failed" | "terminated"
	config: CliAgentConfig
	createdAt: number
	tokenUsage: TokenUsage
}

export interface AgentSnapshot {
	state: AgentState
	messageCount: number
	eventCount: number
	lastTask: TaskResult | null
}

export interface AgentRunResponse {
	agent: AgentSnapshot
	result: TaskResult
}

export interface SwarmConfig {
	strategy: SwarmStrategy
	manager: CliAgentConfig
	workers: CliAgentConfig[]
	maxConcurrentAgents?: number
	maxTurns?: number
	tokenBudget?: number
}

export interface SwarmState {
	id: string
	status: "idle" | "running" | "completed" | "failed"
	agentIds: string[]
	results: TaskResult[]
	tokenUsage: TokenUsage
	startedAt?: number
	completedAt?: number
}

export interface SwarmRunResponse {
	swarm: SwarmState
	result: TaskResult
}

type HeaderRecord = Record<string, string>
type HeaderMap = Record<string, string | readonly string[]>
type RequestHeaders = HeaderRecord | HeaderMap | Headers | Array<[string, string] | string[]>

export interface RunTaskOptions {
	model: string
	provider: string
	name: string
	strategy?: SwarmStrategy
}

export type CliTaskResult =
	| {
		mode: "single"
		agent: AgentSnapshot
		result: TaskResult
	}
	| {
		mode: "swarm"
		swarm: SwarmState
		result: TaskResult
	}

export class DaemonHttpError extends Error {
	readonly status: number
	readonly body: unknown

	constructor(status: number, body: unknown) {
		const message =
			typeof body === "object" &&
			body !== null &&
			"error" in body &&
			typeof body.error === "string"
				? body.error
				: `Daemon request failed with status ${status}`

		super(message)
		this.name = "DaemonHttpError"
		this.status = status
		this.body = body
	}
}

export class CliDaemonClient {
	private readonly baseUrl: string
	private readonly fetchImpl: typeof fetch

	constructor(
		baseUrl: string = process.env.ANIMA_DAEMON_URL ?? DEFAULT_DAEMON_URL,
		fetchImpl: typeof fetch | undefined = globalThis.fetch?.bind(globalThis),
	) {
		if (!fetchImpl) {
			throw new Error("fetch is not available; provide a fetch implementation")
		}

		this.baseUrl = baseUrl.replace(/\/+$/, "")
		this.fetchImpl = fetchImpl
	}

	async runTask(task: string, options: RunTaskOptions): Promise<CliTaskResult> {
		if (options.strategy) {
			const swarm = await this.createSwarm({
				strategy: options.strategy,
				manager: {
					name: "manager",
					model: options.model,
					provider: options.provider,
					system:
						"You are a task manager. Break complex tasks into subtasks and delegate to workers. Synthesize results into a final answer.",
				},
				workers: [
					{
						name: "worker",
						model: options.model,
						provider: options.provider,
						system:
							"You are a helpful worker agent. Complete the assigned task concisely and accurately.",
					},
				],
				maxTurns: 2,
			})
			const run = await this.runSwarm(swarm.id, { text: task })

			return {
				mode: "swarm",
				swarm: run.swarm,
				result: run.result,
			}
		}

		const agent = await this.createAgent({
			name: options.name,
			model: options.model,
			provider: options.provider,
			system: "You are a helpful task agent. Be concise.",
		})
		const run = await this.runAgent(agent.state.id, { text: task })

		return {
			mode: "single",
			agent: run.agent,
			result: run.result,
		}
	}

	async createAgent(config: CliAgentConfig): Promise<AgentSnapshot> {
		const response = await this.requestJson<{ agent: AgentSnapshot }>("/api/agents", {
			method: "POST",
			body: config,
		})
		return response.agent
	}

	async listAgents(): Promise<AgentSnapshot[]> {
		const response = await this.requestJson<{ agents: AgentSnapshot[] }>("/api/agents")
		return response.agents
	}

	async getAgent(agentId: string): Promise<AgentSnapshot> {
		const response = await this.requestJson<{ agent: AgentSnapshot }>(`/api/agents/${agentId}`)
		return response.agent
	}

	async runAgent(agentId: string, input: Content): Promise<AgentRunResponse> {
		return this.requestJson<AgentRunResponse>(`/api/agents/${agentId}/run`, {
			method: "POST",
			body: input,
		})
	}

	async createAgencySwarm(agency: AgencyConfig): Promise<SwarmState> {
		return this.createSwarm({
			strategy: agency.strategy,
			manager: toAgencyAgentConfig(agency.orchestrator, agency.model, agency.provider),
			workers: agency.agents.map((agent) =>
				toAgencyAgentConfig(agent, agency.model, agency.provider),
			),
		})
	}

	async createSwarm(config: SwarmConfig): Promise<SwarmState> {
		const response = await this.requestJson<{ swarm: SwarmState }>("/api/swarms", {
			method: "POST",
			body: config,
		})
		return response.swarm
	}

	async runSwarm(swarmId: string, input: Content): Promise<SwarmRunResponse> {
		return this.requestJson<SwarmRunResponse>(`/api/swarms/${swarmId}/run`, {
			method: "POST",
			body: input,
		})
	}

	private async requestJson<T>(
		path: string,
		init: Omit<RequestInit, "body"> & { body?: unknown } = {},
	): Promise<T> {
		const response = await this.fetchImpl(this.url(path), {
			...init,
			headers: {
				accept: "application/json",
				"content-type": "application/json",
				...toHeaderRecord(init.headers),
			},
			body: init.body === undefined ? undefined : JSON.stringify(init.body),
		})

		const text = await response.text()
		const payload = text.length === 0 ? null : parseJsonLike(text)

		if (!response.ok) {
			throw new DaemonHttpError(response.status, payload)
		}

		return payload as T
	}

	private url(path: string): string {
		return `${this.baseUrl}${path.startsWith("/") ? path : `/${path}`}`
	}
}

export function createCliDaemonClient(): CliDaemonClient {
	return new CliDaemonClient()
}

function toAgencyAgentConfig(
	agent: AgentDefinition,
	defaultModel: string,
	provider: string,
): CliAgentConfig {
	return {
		name: agent.name,
		model: agent.model ?? defaultModel,
		provider,
		system: agent.system,
		bio: agent.bio,
		lore: agent.lore,
		knowledge: agent.knowledge,
		topics: agent.topics,
		adjectives: agent.adjectives,
		style: agent.style,
	}
}

function parseJsonLike(value: string): unknown {
	try {
		return JSON.parse(value) as unknown
	} catch {
		return value
	}
}

function toHeaderRecord(headers?: RequestHeaders): HeaderRecord {
	if (!headers) {
		return {}
	}

	if (headers instanceof Headers) {
		return Object.fromEntries(headers.entries())
	}

	if (Array.isArray(headers)) {
		return Object.fromEntries(headers.map((entry) => [entry[0] ?? "", entry[1] ?? ""]))
	}

	return Object.fromEntries(
		Object.entries(headers).map(([key, value]) => [
			key,
			typeof value === "string" ? value : value.join(", "),
		]),
	)
}
