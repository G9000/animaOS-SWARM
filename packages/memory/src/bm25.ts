// BM25 search engine with simple stemming
// Inspired by elizaOS search.ts (MIT licensed)

const STOP_WORDS = new Set([
	"a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for",
	"of", "with", "by", "is", "are", "was", "were", "be", "been", "being",
	"have", "has", "had", "do", "does", "did", "will", "would", "could",
	"should", "may", "might", "can", "this", "that", "these", "those",
	"it", "its", "i", "you", "he", "she", "we", "they", "me", "him",
	"her", "us", "them", "my", "your", "his", "our", "their", "not", "no",
])

function simpleStem(word: string): string {
	if (word.length < 4) return word
	// Basic suffix stripping
	if (word.endsWith("ing")) return word.slice(0, -3)
	if (word.endsWith("tion")) return word.slice(0, -4)
	if (word.endsWith("ness")) return word.slice(0, -4)
	if (word.endsWith("ment")) return word.slice(0, -4)
	if (word.endsWith("able")) return word.slice(0, -4)
	if (word.endsWith("ible")) return word.slice(0, -4)
	if (word.endsWith("ally")) return word.slice(0, -4)
	if (word.endsWith("ies")) return word.slice(0, -3) + "y"
	if (word.endsWith("ed")) return word.slice(0, -2)
	if (word.endsWith("ly")) return word.slice(0, -2)
	if (word.endsWith("er")) return word.slice(0, -2)
	if (word.endsWith("es")) return word.slice(0, -2)
	if (word.endsWith("s") && !word.endsWith("ss")) return word.slice(0, -1)
	return word
}

function tokenize(text: string): string[] {
	return text
		.toLowerCase()
		.replace(/[^a-z0-9\s]/g, " ")
		.split(/\s+/)
		.filter((w) => w.length > 1 && !STOP_WORDS.has(w))
		.map(simpleStem)
}

export interface SearchResult {
	id: string
	score: number
}

interface DocEntry {
	id: string
	text: string
	terms: string[]
	termFreqs: Map<string, number>
}

export class BM25 {
	private docs = new Map<string, DocEntry>()
	private docFreq = new Map<string, number>() // term → number of docs containing it
	private avgDocLen = 0
	private k1: number
	private b: number

	constructor(k1 = 1.5, b = 0.75) {
		this.k1 = k1
		this.b = b
	}

	addDocument(id: string, text: string): void {
		// Remove if exists
		if (this.docs.has(id)) {
			this.removeDocument(id)
		}

		const terms = tokenize(text)
		const termFreqs = new Map<string, number>()
		for (const term of terms) {
			termFreqs.set(term, (termFreqs.get(term) ?? 0) + 1)
		}

		// Update doc frequency
		for (const term of termFreqs.keys()) {
			this.docFreq.set(term, (this.docFreq.get(term) ?? 0) + 1)
		}

		this.docs.set(id, { id, text, terms, termFreqs })
		this.updateAvgLen()
	}

	removeDocument(id: string): void {
		const doc = this.docs.get(id)
		if (!doc) return

		// Decrease doc frequency
		for (const term of doc.termFreqs.keys()) {
			const count = this.docFreq.get(term) ?? 1
			if (count <= 1) {
				this.docFreq.delete(term)
			} else {
				this.docFreq.set(term, count - 1)
			}
		}

		this.docs.delete(id)
		this.updateAvgLen()
	}

	search(query: string, limit = 10): SearchResult[] {
		const queryTerms = tokenize(query)
		if (queryTerms.length === 0 || this.docs.size === 0) return []

		const N = this.docs.size
		const scores: SearchResult[] = []

		for (const doc of this.docs.values()) {
			let score = 0
			const docLen = doc.terms.length

			for (const term of queryTerms) {
				const tf = doc.termFreqs.get(term) ?? 0
				if (tf === 0) continue

				const df = this.docFreq.get(term) ?? 0
				// BM25 IDF: log(1 + (N - df + 0.5) / (df + 0.5))
				const idf = Math.log(1 + (N - df + 0.5) / (df + 0.5))
				// BM25 TF normalization
				const tfNorm = (tf * (this.k1 + 1)) / (tf + this.k1 * (1 - this.b + this.b * (docLen / this.avgDocLen)))
				score += idf * tfNorm
			}

			if (score > 0) {
				scores.push({ id: doc.id, score })
			}
		}

		return scores.sort((a, b) => b.score - a.score).slice(0, limit)
	}

	clear(): void {
		this.docs.clear()
		this.docFreq.clear()
		this.avgDocLen = 0
	}

	get size(): number {
		return this.docs.size
	}

	private updateAvgLen(): void {
		if (this.docs.size === 0) {
			this.avgDocLen = 0
			return
		}
		let total = 0
		for (const doc of this.docs.values()) {
			total += doc.terms.length
		}
		this.avgDocLen = total / this.docs.size
	}
}
