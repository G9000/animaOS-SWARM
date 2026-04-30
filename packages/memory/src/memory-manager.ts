import { randomUUID } from "node:crypto"
import { readFileSync, writeFileSync, existsSync } from "node:fs"
import { BM25 } from "./bm25.js"

export type MemoryType = "fact" | "observation" | "task_result" | "reflection"
export type MemoryScope = "shared" | "private" | "room"
export type RelationshipEndpointKind = "agent" | "user" | "system" | "external"

const RELATIONSHIP_ENDPOINT_KINDS = new Set<RelationshipEndpointKind>(["agent", "user", "system", "external"])

export interface AgentRelationship {
	id: string
	sourceKind: RelationshipEndpointKind
	sourceAgentId: string
	sourceAgentName: string
	targetKind: RelationshipEndpointKind
	targetAgentId: string
	targetAgentName: string
	relationshipType: string
	summary?: string
	strength: number
	confidence: number
	evidenceMemoryIds: string[]
	tags?: string[]
	roomId?: string
	worldId?: string
	sessionId?: string
	createdAt: number
	updatedAt: number
}

export interface NewAgentRelationshipInput {
	sourceKind?: RelationshipEndpointKind
	sourceAgentId: string
	sourceAgentName: string
	targetKind?: RelationshipEndpointKind
	targetAgentId: string
	targetAgentName: string
	relationshipType: string
	summary?: string
	strength?: number
	confidence?: number
	evidenceMemoryIds?: string[]
	tags?: string[]
	roomId?: string
	worldId?: string
	sessionId?: string
}

export interface AgentRelationshipOptions {
	entityId?: string
	agentId?: string
	sourceKind?: RelationshipEndpointKind
	sourceAgentId?: string
	targetKind?: RelationshipEndpointKind
	targetAgentId?: string
	relationshipType?: string
	roomId?: string
	worldId?: string
	sessionId?: string
	minStrength?: number
	minConfidence?: number
	limit?: number
}

export interface Memory {
	id: string
	agentId: string
	agentName: string
	type: MemoryType
	content: string
	importance: number // 0-1
	createdAt: number
	tags?: string[]
	scope: MemoryScope
	roomId?: string
	worldId?: string
	sessionId?: string
}

export interface MemorySearchResult extends Memory {
	score: number
}

export interface MemorySearchOptions {
	agentId?: string
	agentName?: string
	type?: MemoryType
	scope?: MemoryScope
	roomId?: string
	worldId?: string
	sessionId?: string
	limit?: number
	minImportance?: number
}

export type NewMemoryInput = Omit<Memory, "id" | "createdAt" | "scope"> & {
	scope?: MemoryScope
}

export class MemoryManager {
	private memories = new Map<string, Memory>()
	private agentRelationships = new Map<string, AgentRelationship>()
	private index = new BM25()
	private storageFile?: string

	constructor(storageFile?: string) {
		this.storageFile = storageFile
	}

	add(memory: NewMemoryInput): Memory {
		const full: Memory = {
			...memory,
			scope: memory.scope ?? (memory.roomId ? "room" : "private"),
			id: randomUUID(),
			createdAt: Date.now(),
		}
		this.memories.set(full.id, full)
		// Index: content + type + tags for BM25 search
		const indexText = [
			full.content,
			full.type,
			full.scope,
			full.agentName,
			full.roomId,
			full.worldId,
			full.sessionId,
			...(full.tags ?? []),
		].filter(Boolean).join(" ")
		this.index.addDocument(full.id, indexText)
		return full
	}

	search(query: string, opts: MemorySearchOptions = {}): MemorySearchResult[] {
		const { agentId, agentName, type, scope, roomId, worldId, sessionId, limit = 10, minImportance = 0 } = opts
		const raw = this.index.search(query, limit * 3) // over-fetch then filter

		const results: MemorySearchResult[] = []
		for (const r of raw) {
			const mem = this.memories.get(r.id)
			if (!mem) continue
			if (agentId && mem.agentId !== agentId) continue
			if (agentName && mem.agentName !== agentName) continue
			if (type && mem.type !== type) continue
			if (scope && mem.scope !== scope) continue
			if (roomId && mem.roomId !== roomId) continue
			if (worldId && mem.worldId !== worldId) continue
			if (sessionId && mem.sessionId !== sessionId) continue
			if (mem.importance < minImportance) continue
			results.push({ ...mem, score: r.score })
			if (results.length >= limit) break
		}
		return results
	}

	getRecent(opts: { agentId?: string; agentName?: string; scope?: MemoryScope; roomId?: string; worldId?: string; sessionId?: string; limit?: number } = {}): Memory[] {
		const { agentId, agentName, scope, roomId, worldId, sessionId, limit = 20 } = opts
		return Array.from(this.memories.values())
			.filter((m) => {
				if (agentId && m.agentId !== agentId) return false
				if (agentName && m.agentName !== agentName) return false
				if (scope && m.scope !== scope) return false
				if (roomId && m.roomId !== roomId) return false
				if (worldId && m.worldId !== worldId) return false
				if (sessionId && m.sessionId !== sessionId) return false
				return true
			})
			.sort((a, b) => b.createdAt - a.createdAt)
			.slice(0, limit)
	}

	upsertAgentRelationship(input: NewAgentRelationshipInput): AgentRelationship {
		const sourceKind = normalizeRelationshipEndpointKind(input.sourceKind, "sourceKind")
		const sourceAgentId = normalizeRequiredString(input.sourceAgentId, "sourceAgentId")
		const sourceAgentName = normalizeRequiredString(input.sourceAgentName, "sourceAgentName")
		const targetKind = normalizeRelationshipEndpointKind(input.targetKind, "targetKind")
		const targetAgentId = normalizeRequiredString(input.targetAgentId, "targetAgentId")
		const targetAgentName = normalizeRequiredString(input.targetAgentName, "targetAgentName")
		const relationshipType = normalizeRequiredString(input.relationshipType, "relationshipType")
		const strength = input.strength ?? 0.5
		const confidence = input.confidence ?? 0.5
		validateUnitInterval(strength, "strength")
		validateUnitInterval(confidence, "confidence")

		const existing = Array.from(this.agentRelationships.values()).find((relationship) =>
			relationship.sourceKind === sourceKind &&
			relationship.sourceAgentId === sourceAgentId &&
			relationship.targetKind === targetKind &&
			relationship.targetAgentId === targetAgentId &&
			relationship.relationshipType === relationshipType &&
			relationship.worldId === input.worldId
		)
		const now = Date.now()

		if (existing) {
			const updated: AgentRelationship = {
				...existing,
				sourceAgentName,
				targetAgentName,
				summary: input.summary ?? existing.summary,
				strength,
				confidence,
				evidenceMemoryIds: unique([...existing.evidenceMemoryIds, ...(input.evidenceMemoryIds ?? [])]),
				tags: unique([...(existing.tags ?? []), ...(input.tags ?? [])]),
				roomId: input.roomId ?? existing.roomId,
				sessionId: input.sessionId ?? existing.sessionId,
				updatedAt: now,
			}
			if (updated.tags?.length === 0) delete updated.tags
			this.agentRelationships.set(updated.id, updated)
			return updated
		}

		const relationship: AgentRelationship = {
			id: randomUUID(),
			sourceKind,
			sourceAgentId,
			sourceAgentName,
			targetKind,
			targetAgentId,
			targetAgentName,
			relationshipType,
			summary: input.summary,
			strength,
			confidence,
			evidenceMemoryIds: unique(input.evidenceMemoryIds ?? []),
			tags: input.tags ? unique(input.tags) : undefined,
			roomId: input.roomId,
			worldId: input.worldId,
			sessionId: input.sessionId,
			createdAt: now,
			updatedAt: now,
		}
		if (relationship.tags?.length === 0) delete relationship.tags
		this.agentRelationships.set(relationship.id, relationship)
		return relationship
	}

	listAgentRelationships(opts: AgentRelationshipOptions = {}): AgentRelationship[] {
		const { entityId, agentId, sourceKind, sourceAgentId, targetKind, targetAgentId, relationshipType, roomId, worldId, sessionId, minStrength = 0, minConfidence = 0, limit = 20 } = opts
		return Array.from(this.agentRelationships.values())
			.filter((relationship) => {
				if (entityId && relationship.sourceAgentId !== entityId && relationship.targetAgentId !== entityId) return false
				if (agentId && (relationship.sourceKind !== "agent" || relationship.sourceAgentId !== agentId) && (relationship.targetKind !== "agent" || relationship.targetAgentId !== agentId)) return false
				if (sourceKind && relationship.sourceKind !== sourceKind) return false
				if (sourceAgentId && relationship.sourceAgentId !== sourceAgentId) return false
				if (targetKind && relationship.targetKind !== targetKind) return false
				if (targetAgentId && relationship.targetAgentId !== targetAgentId) return false
				if (relationshipType && relationship.relationshipType !== relationshipType) return false
				if (roomId && relationship.roomId !== roomId) return false
				if (worldId && relationship.worldId !== worldId) return false
				if (sessionId && relationship.sessionId !== sessionId) return false
				if (relationship.strength < minStrength) return false
				if (relationship.confidence < minConfidence) return false
				return true
			})
			.sort((a, b) => b.strength - a.strength || b.updatedAt - a.updatedAt)
			.slice(0, limit)
	}

	forgetAgentRelationship(id: string): void {
		this.agentRelationships.delete(id)
	}

	forget(id: string): void {
		this.memories.delete(id)
		this.index.removeDocument(id)
	}

	clear(agentId?: string): void {
		if (!agentId) {
			this.memories.clear()
			this.agentRelationships.clear()
			this.index.clear()
			return
		}
		for (const [id, mem] of this.memories) {
			if (mem.agentId === agentId) {
				this.memories.delete(id)
				this.index.removeDocument(id)
			}
		}
		for (const [id, relationship] of this.agentRelationships) {
			if ((relationship.sourceKind === "agent" && relationship.sourceAgentId === agentId) || (relationship.targetKind === "agent" && relationship.targetAgentId === agentId)) {
				this.agentRelationships.delete(id)
			}
		}
	}

	save(): void {
		if (!this.storageFile) return
		const data = {
			version: 1,
			memories: Array.from(this.memories.values()),
			agentRelationships: Array.from(this.agentRelationships.values()),
		}
		writeFileSync(this.storageFile, JSON.stringify(data, null, 2))
	}

	load(): void {
		if (!this.storageFile || !existsSync(this.storageFile)) return
		try {
			const raw = readFileSync(this.storageFile, "utf-8")
			const data = JSON.parse(raw) as Memory[] | { memories?: Memory[]; agentRelationships?: Partial<AgentRelationship>[] }
			const memories = Array.isArray(data) ? data : data.memories ?? []
			const agentRelationships = Array.isArray(data) ? [] : data.agentRelationships ?? []
			for (const mem of memories) {
				mem.scope ??= mem.roomId ? "room" : "private"
				this.memories.set(mem.id, mem)
				const indexText = [mem.content, mem.type, mem.scope, mem.agentName, mem.roomId, mem.worldId, mem.sessionId, ...(mem.tags ?? [])].filter(Boolean).join(" ")
				this.index.addDocument(mem.id, indexText)
			}
			for (const relationship of agentRelationships) {
				const normalized = normalizeStoredRelationship(relationship)
				if (normalized) this.agentRelationships.set(normalized.id, normalized)
			}
		} catch {
			// Corrupted file — start fresh
		}
	}

	get size(): number {
		return this.memories.size
	}

	get relationshipCount(): number {
		return this.agentRelationships.size
	}

	/** Return a summary line for display (e.g. TUI status) */
	summary(): string {
		return `${this.memories.size} memories`
	}
}

function validateUnitInterval(value: number, field: string): void {
	if (!Number.isFinite(value) || value < 0 || value > 1) {
		throw new Error(`${field} must be between 0 and 1`)
	}
}

function isRelationshipEndpointKind(value: unknown): value is RelationshipEndpointKind {
	return typeof value === "string" && RELATIONSHIP_ENDPOINT_KINDS.has(value as RelationshipEndpointKind)
}

function normalizeRelationshipEndpointKind(value: RelationshipEndpointKind | undefined, field: string): RelationshipEndpointKind {
	if (value === undefined) return "agent"
	if (isRelationshipEndpointKind(value)) return value
	throw new Error(`${field} must be one of agent, user, system, external`)
}

function normalizeStoredRelationship(input: Partial<AgentRelationship>): AgentRelationship | undefined {
	const sourceKind = input.sourceKind === undefined ? "agent" : input.sourceKind
	const targetKind = input.targetKind === undefined ? "agent" : input.targetKind
	const strength = input.strength
	const confidence = input.confidence
	const createdAt = input.createdAt
	const updatedAt = input.updatedAt
	if (!isRelationshipEndpointKind(sourceKind) || !isRelationshipEndpointKind(targetKind)) return undefined
	if (!input.id || !input.sourceAgentId || !input.sourceAgentName || !input.targetAgentId || !input.targetAgentName || !input.relationshipType) return undefined
	if (!isUnitInterval(strength) || !isUnitInterval(confidence) || !isFiniteNumber(createdAt) || !isFiniteNumber(updatedAt)) return undefined

	return {
		id: input.id,
		sourceKind,
		sourceAgentId: input.sourceAgentId,
		sourceAgentName: input.sourceAgentName,
		targetKind,
		targetAgentId: input.targetAgentId,
		targetAgentName: input.targetAgentName,
		relationshipType: input.relationshipType,
		summary: input.summary,
		strength,
		confidence,
		evidenceMemoryIds: Array.isArray(input.evidenceMemoryIds) ? unique(input.evidenceMemoryIds) : [],
		tags: Array.isArray(input.tags) ? unique(input.tags) : undefined,
		roomId: input.roomId,
		worldId: input.worldId,
		sessionId: input.sessionId,
		createdAt,
		updatedAt,
	}
}

function isFiniteNumber(value: unknown): value is number {
	return typeof value === "number" && Number.isFinite(value)
}

function isUnitInterval(value: unknown): value is number {
	return isFiniteNumber(value) && value >= 0 && value <= 1
}

function normalizeRequiredString(value: string, field: string): string {
	const normalized = value.trim()
	if (!normalized) throw new Error(`${field} must not be empty`)
	return normalized
}

function unique(values: string[]): string[] {
	return Array.from(new Set(values.map((value) => value.trim()).filter(Boolean)))
}
