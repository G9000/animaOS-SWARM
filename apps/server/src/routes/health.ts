import { route, json } from "../server.js"

export const healthRoutes = [
	route("GET", "/api/health", async (_req, res, state) => {
		json(res, 200, {
			status: "ok",
			agents: state.agents.size,
			swarms: state.swarms.size,
			uptime: process.uptime(),
		})
	}),
]
