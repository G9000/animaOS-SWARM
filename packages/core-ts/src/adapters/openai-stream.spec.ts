import { describe, it, expect } from "vitest"
import { OpenAIAdapter } from "./openai.js"

describe("OpenAIAdapter.generateStream", () => {
	it("should be defined as a method", () => {
		const adapter = new OpenAIAdapter("test-key")
		expect(adapter.generateStream).toBeDefined()
		expect(typeof adapter.generateStream).toBe("function")
	})
})
