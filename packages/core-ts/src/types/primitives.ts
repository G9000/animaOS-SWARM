export type UUID = `${string}-${string}-${string}-${string}-${string}`

export interface Content {
	text: string
	attachments?: Attachment[]
	metadata?: Record<string, unknown>
}

export interface Attachment {
	type: "file" | "image" | "url"
	name: string
	data: string
}

export interface Message {
	id: UUID
	agentId: UUID
	roomId: UUID
	content: Content
	role: "user" | "assistant" | "system" | "tool"
	createdAt: number
}

export interface TaskResult<T = unknown> {
	status: "success" | "error"
	data?: T
	error?: string
	durationMs: number
}
