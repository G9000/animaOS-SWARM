import { describe, it, expect, vi } from "vitest"
import { EventBus } from "./event-bus.js"

describe("EventBus", () => {
	it("should emit and receive events", async () => {
		const bus = new EventBus()
		const handler = vi.fn()

		bus.on("agent:spawned", handler)
		await bus.emit("agent:spawned", { name: "test" })

		expect(handler).toHaveBeenCalledOnce()
		const event = handler.mock.calls[0][0]
		expect(event.id).toEqual(expect.any(String))
		expect(event.timestampMs).toEqual(expect.any(Number))
		expect(event.data).toEqual({ name: "test" })
	})

	it("should unsubscribe", async () => {
		const bus = new EventBus()
		const handler = vi.fn()

		const unsub = bus.on("agent:spawned", handler)
		unsub()
		await bus.emit("agent:spawned", { name: "test" })

		expect(handler).not.toHaveBeenCalled()
	})

	it("should handle errors in handlers gracefully", async () => {
		const bus = new EventBus()
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {})

		bus.on("task:started", () => { throw new Error("boom") })
		await bus.emit("task:started", {})

		expect(errorSpy).toHaveBeenCalled()
		errorSpy.mockRestore()
	})

	it("should clear all listeners", async () => {
		const bus = new EventBus()
		const handler = vi.fn()

		bus.on("agent:spawned", handler)
		bus.clear()
		await bus.emit("agent:spawned", {})

		expect(handler).not.toHaveBeenCalled()
	})
})
