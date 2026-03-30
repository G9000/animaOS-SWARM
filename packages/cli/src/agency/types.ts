export interface AgentDefinition {
	name: string
	bio: string
	system: string
	role?: "orchestrator" | "worker"
	model?: string
	tools?: string[]
}

export interface AgencyConfig {
	name: string
	description: string
	model: string
	provider: string
	strategy: "supervisor" | "dynamic" | "round-robin"
	orchestrator: AgentDefinition
	agents: AgentDefinition[]
}
