import { route, json } from "../server.js"
import type { SwarmConfig } from "@animaOS-SWARM/swarm"

export const swarmRoutes = [
	// Create swarm
	route("POST", "/api/swarms", async (_req, res, state, body) => {
		const config = body as SwarmConfig
		if (!config.strategy || !config.manager || !config.workers) {
			json(res, 400, { error: "strategy, manager, and workers are required" })
			return
		}
		const swarm = state.createSwarm(config)
		json(res, 201, { id: swarm.id, strategy: config.strategy })
	}),

	// List swarms
	route("GET", "/api/swarms", async (_req, res, state) => {
		const swarms = Array.from(state.swarms.values()).map((s) => s.getState())
		json(res, 200, { swarms })
	}),

	// Get swarm
	route("GET", "/api/swarms/:id", async (_req, res, state, _body, params) => {
		const swarm = state.swarms.get(params.id)
		if (!swarm) {
			json(res, 404, { error: "Swarm not found" })
			return
		}
		json(res, 200, swarm.getState())
	}),

	// Run task on swarm
	route("POST", "/api/swarms/:id/run", async (_req, res, state, body, params) => {
		const swarm = state.swarms.get(params.id)
		if (!swarm) {
			json(res, 404, { error: "Swarm not found" })
			return
		}
		const task = body.task as string
		if (!task) {
			json(res, 400, { error: "task is required" })
			return
		}
		const result = await swarm.run(task)
		json(res, 200, result)
	}),
]
