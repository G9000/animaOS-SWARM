import { BM25 } from "./bm25.js"

export interface DocumentChunk {
	docId: string
	chunkId: string
	text: string
	metadata?: Record<string, unknown>
}

export interface DocumentSearchResult {
	docId: string
	chunkId: string
	text: string
	score: number
	metadata?: Record<string, unknown>
}

function chunkText(text: string, maxChunkSize = 500): string[] {
	// Split by paragraphs first
	const paragraphs = text.split(/\n\n+/).filter((p) => p.trim().length > 0)
	const chunks: string[] = []

	for (const para of paragraphs) {
		if (para.length <= maxChunkSize) {
			chunks.push(para.trim())
		} else {
			// Split long paragraphs by sentences
			const sentences = para.split(/(?<=[.!?])\s+/)
			let current = ""
			for (const sentence of sentences) {
				if (current.length + sentence.length > maxChunkSize && current.length > 0) {
					chunks.push(current.trim())
					current = sentence
				} else {
					current += (current ? " " : "") + sentence
				}
			}
			if (current.trim()) {
				chunks.push(current.trim())
			}
		}
	}

	return chunks.length > 0 ? chunks : [text.trim()]
}

export class DocumentStore {
	private index = new BM25()
	private chunks = new Map<string, DocumentChunk>()
	private docChunks = new Map<string, string[]>() // docId → chunkIds

	ingest(docId: string, text: string, metadata?: Record<string, unknown>): number {
		// Remove existing chunks for this doc
		this.remove(docId)

		const textChunks = chunkText(text)
		const chunkIds: string[] = []

		for (let i = 0; i < textChunks.length; i++) {
			const chunkId = `${docId}:${i}`
			const chunk: DocumentChunk = { docId, chunkId, text: textChunks[i], metadata }

			this.chunks.set(chunkId, chunk)
			this.index.addDocument(chunkId, textChunks[i])
			chunkIds.push(chunkId)
		}

		this.docChunks.set(docId, chunkIds)
		return textChunks.length
	}

	search(query: string, limit = 10): DocumentSearchResult[] {
		const results = this.index.search(query, limit)
		const out: DocumentSearchResult[] = []
		for (const r of results) {
			const chunk = this.chunks.get(r.id)
			if (!chunk) continue
			out.push({
				docId: chunk.docId,
				chunkId: chunk.chunkId,
				text: chunk.text,
				score: r.score,
				metadata: chunk.metadata,
			})
		}
		return out
	}

	remove(docId: string): void {
		const chunkIds = this.docChunks.get(docId)
		if (!chunkIds) return

		for (const chunkId of chunkIds) {
			this.chunks.delete(chunkId)
			this.index.removeDocument(chunkId)
		}
		this.docChunks.delete(docId)
	}

	clear(): void {
		this.chunks.clear()
		this.docChunks.clear()
		this.index.clear()
	}

	get documentCount(): number {
		return this.docChunks.size
	}

	get chunkCount(): number {
		return this.chunks.size
	}
}
