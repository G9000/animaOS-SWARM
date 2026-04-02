import { AgentsClient } from "./agents.js"
import { SwarmsClient } from "./swarms.js"

const DEFAULT_BASE_URL = "http://127.0.0.1:3000"

export type FetchLike = typeof fetch
type RequestHeaders = NonNullable<RequestInit["headers"]>
type RequestBody = NonNullable<RequestInit["body"]>

export interface DaemonClientOptions {
	baseUrl?: string
	fetch?: FetchLike
}

export interface DaemonEvent<T = unknown> {
	event: string
	data: T
	id?: string
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

export class DaemonClient {
	readonly agents: AgentsClient
	readonly swarms: SwarmsClient

	private readonly baseUrl: string
	private readonly fetchImpl: FetchLike

	constructor(options: DaemonClientOptions = {}) {
		this.baseUrl = normalizeBaseUrl(options.baseUrl ?? DEFAULT_BASE_URL)
		const fetchImpl = options.fetch ?? globalThis.fetch?.bind(globalThis)

		if (!fetchImpl) {
			throw new Error("fetch is not available; provide a fetch implementation")
		}

		this.fetchImpl = fetchImpl
		this.agents = new AgentsClient(this)
		this.swarms = new SwarmsClient(this)
	}

	async requestJson<T>(
		path: string,
		init: Omit<RequestInit, "body"> & {
			body?: unknown
		} = {},
	): Promise<T> {
		const { body, isJson } = prepareRequestBody(init.body)
		const headers: Record<string, string> = {
			accept: "application/json",
			...headersToObject(init.headers),
		}
		if (isJson && !("content-type" in headers)) {
			headers["content-type"] = "application/json"
		}

		const response = await this.fetchImpl(this.url(path), {
			...init,
			headers,
			body,
		})

		const payload = await readResponseBody(response)
		if (!response.ok) {
			throw new DaemonHttpError(response.status, payload)
		}

		return payload as T
	}

	async *subscribe<T = unknown>(
		path: string,
		init: RequestInit = {},
	): AsyncGenerator<DaemonEvent<T>> {
		const abortController = new AbortController()
		const detachAbortRelay = relayAbort(init.signal, abortController)
		const response = await this.fetchImpl(this.url(path), {
			...init,
			method: init.method ?? "GET",
			headers: {
				accept: "text/event-stream",
				...headersToObject(init.headers),
			},
			signal: abortController.signal,
		})

		if (!response.ok) {
			detachAbortRelay()
			throw new DaemonHttpError(response.status, await readResponseBody(response))
		}

		if (!response.body) {
			detachAbortRelay()
			throw new Error("daemon event stream did not include a response body")
		}

		const reader = response.body.getReader()
		const decoder = new TextDecoder()
		let buffer = ""

		try {
			while (true) {
				const { done, value } = await reader.read()
				if (done) {
					break
				}

				buffer += decoder.decode(value, { stream: true })
				let separatorIndex = findEventSeparator(buffer)
				while (separatorIndex >= 0) {
					const chunk = buffer.slice(0, separatorIndex)
					buffer = buffer.slice(
						separatorIndex + eventSeparatorLength(buffer, separatorIndex),
					)

					const event = parseSseEvent(chunk)
					if (event) {
						yield event as DaemonEvent<T>
					}

					separatorIndex = findEventSeparator(buffer)
				}
			}

			buffer += decoder.decode()
			const trailingEvent = parseSseEvent(buffer)
			if (trailingEvent) {
				yield trailingEvent as DaemonEvent<T>
			}
		} finally {
			abortController.abort()
			detachAbortRelay()
			await reader.cancel("subscription closed").catch(() => undefined)
			reader.releaseLock()
		}
	}

	private url(path: string): string {
		return `${this.baseUrl}${path.startsWith("/") ? path : `/${path}`}`
	}
}

export function createDaemonClient(options: DaemonClientOptions = {}): DaemonClient {
	return new DaemonClient(options)
}

function normalizeBaseUrl(baseUrl: string): string {
	return baseUrl.replace(/\/+$/, "")
}

function headersToObject(headers?: RequestHeaders): Record<string, string> {
	if (!headers) {
		return {}
	}

	const normalized: Record<string, string> = {}

	if (headers instanceof Headers) {
		for (const [key, value] of headers.entries()) {
			normalized[key] = value
		}
		return normalized
	}

	if (Array.isArray(headers)) {
		for (const [key, value] of headers) {
			normalized[key] = value
		}
		return normalized
	}

	for (const [key, value] of Object.entries(
		headers as Record<string, string | readonly string[]>,
	)) {
		normalized[key] = typeof value === "string" ? value : value.join(", ")
	}

	return normalized
}

function prepareRequestBody(body: unknown): {
	body: RequestBody | undefined
	isJson: boolean
} {
	if (body === undefined || body === null) {
		return {
			body: undefined,
			isJson: false,
		}
	}

	if (
		typeof body === "string" ||
		body instanceof Blob ||
		body instanceof ArrayBuffer ||
		body instanceof FormData ||
		body instanceof URLSearchParams ||
		body instanceof ReadableStream
	) {
		return {
			body,
			isJson: false,
		}
	}

	return {
		body: JSON.stringify(body),
		isJson: true,
	}
}

async function readResponseBody(response: Response): Promise<unknown> {
	const text = await response.text()
	if (text.length === 0) {
		return null
	}

	try {
		return JSON.parse(text) as unknown
	} catch {
		return text
	}
}

function findEventSeparator(buffer: string): number {
	const windowsIndex = buffer.indexOf("\r\n\r\n")
	const unixIndex = buffer.indexOf("\n\n")

	if (windowsIndex === -1) {
		return unixIndex
	}

	if (unixIndex === -1) {
		return windowsIndex
	}

	return Math.min(windowsIndex, unixIndex)
}

function eventSeparatorLength(buffer: string, index: number): number {
	return buffer.startsWith("\r\n\r\n", index) ? 4 : 2
}

function parseSseEvent(rawEvent: string): DaemonEvent | null {
	const normalized = rawEvent.replace(/\r\n/g, "\n").trim()
	if (normalized.length === 0) {
		return null
	}

	let event = "message"
	let id: string | undefined
	const dataLines: string[] = []

	for (const line of normalized.split("\n")) {
		if (line.length === 0 || line.startsWith(":")) {
			continue
		}

		const separatorIndex = line.indexOf(":")
		const field = separatorIndex === -1 ? line : line.slice(0, separatorIndex)
		const rawValue = separatorIndex === -1 ? "" : line.slice(separatorIndex + 1)
		const value = rawValue.startsWith(" ") ? rawValue.slice(1) : rawValue

		switch (field) {
			case "event":
				event = value
				break
			case "data":
				dataLines.push(value)
				break
			case "id":
				id = value
				break
			default:
				break
		}
	}

	if (dataLines.length === 0) {
		return null
	}

	const rawData = dataLines.join("\n")

	return {
		event,
		data: parseJsonLike(rawData),
		...(id ? { id } : {}),
	}
}

function parseJsonLike(value: string): unknown {
	try {
		return JSON.parse(value) as unknown
	} catch {
		return value
	}
}

function relayAbort(
	source: AbortSignal | null | undefined,
	target: AbortController,
): () => void {
	if (!source) {
		return () => {}
	}

	if (source.aborted) {
		target.abort()
		return () => {}
	}

	const onAbort = () => {
		target.abort()
	}

	source.addEventListener("abort", onAbort, { once: true })

	return () => {
		source.removeEventListener("abort", onAbort)
	}
}
