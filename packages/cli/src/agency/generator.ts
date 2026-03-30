import {
	OpenAIAdapter,
	AnthropicAdapter,
	OllamaAdapter,
	type IModelAdapter,
	type ModelConfig,
	type UUID,
} from "@animaOS-SWARM/core"
import type { AgentDefinition } from "./types.js"

/**
 * Create a model adapter for the given provider.
 */
export function createAdapter(provider: string, apiKey?: string): IModelAdapter {
	switch (provider) {
		case "openai":
			return new OpenAIAdapter(apiKey)
		case "anthropic":
			return new AnthropicAdapter(apiKey)
		case "ollama":
			return new OllamaAdapter()
		default:
			throw new Error(`Unsupported provider: ${provider}`)
	}
}

export interface GenerateAgentTeamOptions {
	adapter: IModelAdapter
	model: string
	agencyName: string
	agencyDescription: string
}

/**
 * Use an LLM to generate 2-4 worker agent suggestions for an agency
 * based on its name, description, and orchestrator bio.
 */
export async function generateAgentTeam(
	opts: GenerateAgentTeamOptions,
): Promise<AgentDefinition[]> {
	const dummyId = "00000000-0000-0000-0000-000000000000" as UUID

	const config: ModelConfig = {
		provider: opts.adapter.provider,
		model: opts.model,
	}

	const prompt = [
		`You are designing a team of AI agents for an agency called "${opts.agencyName}".`,
		`Agency purpose: ${opts.agencyDescription}`,
		"",
		"Suggest 3-5 agents (including an orchestrator) that would form this agency.",
		"The first agent should be the orchestrator — the one who coordinates the team.",
		"The rest are workers with distinct roles.",
		"",
		"Respond with ONLY valid JSON — an array of agent objects. No markdown, no explanation.",
		"Each object must have these fields:",
		'  - "name": a short snake_case identifier',
		'  - "bio": who this agent is — personality, expertise, what makes them good at their role (1-2 sentences)',
		'  - "system": the system prompt instruction for this agent (2-3 sentences)',
		'  - "role": either "orchestrator" or "worker"',
	].join("\n")

	const result = await opts.adapter.generate(config, {
		system: "You are a helpful assistant that outputs only valid JSON.",
		messages: [
			{
				id: dummyId,
				agentId: dummyId,
				roomId: dummyId,
				content: { text: prompt },
				role: "user",
				createdAt: Date.now(),
			},
		],
	})

	const text = result.content.text.trim()

	// Strip markdown code fences if the LLM wrapped the response
	const cleaned = text
		.replace(/^```(?:json)?\s*\n?/i, "")
		.replace(/\n?```\s*$/i, "")
		.trim()

	let parsed: unknown
	try {
		parsed = JSON.parse(cleaned)
	} catch {
		throw new Error(
			`Failed to parse LLM response as JSON. Raw response:\n${text}`,
		)
	}

	if (!Array.isArray(parsed)) {
		throw new Error(
			`Expected JSON array from LLM, got ${typeof parsed}`,
		)
	}

	return parsed.map((item: Record<string, unknown>) => ({
		name: (item.name as string) ?? "unnamed",
		bio: (item.bio as string) ?? "",
		system: (item.system as string) ?? "",
		role: ((item.role as string) ?? "worker") as "orchestrator" | "worker",
	}))
}
