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
		'  - "role": either "orchestrator" or "worker"',
		'  - "bio": 1-2 sentences describing who this agent is — personality and expertise',
		'  - "lore": 1-2 sentences of backstory — what shaped them, their origin',
		'  - "adjectives": array of 3-5 personality trait words (e.g. ["analytical", "thorough", "methodical"])',
		'  - "topics": array of 3-6 short expertise tags (e.g. ["web research", "data analysis", "fact checking"])',
		'  - "knowledge": array of 2-4 specific things this agent knows deeply',
		'  - "style": 1-2 sentences describing how this agent communicates',
		'  - "system": core instruction — what they do and how (2-3 sentences)',
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
		role: ((item.role as string) ?? "worker") as "orchestrator" | "worker",
		bio: (item.bio as string) ?? "",
		lore: (item.lore as string | undefined),
		adjectives: Array.isArray(item.adjectives) ? (item.adjectives as string[]) : undefined,
		topics: Array.isArray(item.topics) ? (item.topics as string[]) : undefined,
		knowledge: Array.isArray(item.knowledge) ? (item.knowledge as string[]) : undefined,
		style: (item.style as string | undefined),
		system: (item.system as string) ?? "",
	}))
}
