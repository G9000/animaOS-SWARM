import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from "node:http"
import { agentRoutes } from "./routes/agents.js"
import { swarmRoutes } from "./routes/swarms.js"
import { searchRoutes } from "./routes/search.js"
import { healthRoutes } from "./routes/health.js"
import { AppState } from "./state.js"

function parseBody(req: IncomingMessage): Promise<Record<string, unknown>> {
	return new Promise((resolve, reject) => {
		let body = ""
		req.on("data", (chunk: Buffer) => { body += chunk.toString() })
		req.on("end", () => {
			try {
				resolve(body ? JSON.parse(body) : {})
			} catch {
				reject(new Error("Invalid JSON"))
			}
		})
		req.on("error", reject)
	})
}

function json(res: ServerResponse, status: number, data: unknown) {
	res.writeHead(status, { "Content-Type": "application/json", "Access-Control-Allow-Origin": "*" })
	res.end(JSON.stringify(data))
}

function cors(res: ServerResponse) {
	res.setHeader("Access-Control-Allow-Origin", "*")
	res.setHeader("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS")
	res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization")
}

export type RouteHandler = (req: IncomingMessage, res: ServerResponse, state: AppState, body: Record<string, unknown>, params: Record<string, string>) => Promise<void>

export interface Route {
	method: string
	pattern: RegExp
	paramNames: string[]
	handler: RouteHandler
}

function matchRoute(routes: Route[], method: string, url: string): { route: Route; params: Record<string, string> } | null {
	for (const route of routes) {
		if (route.method !== method) continue
		const match = url.match(route.pattern)
		if (match) {
			const params: Record<string, string> = {}
			route.paramNames.forEach((name, i) => {
				params[name] = match[i + 1]
			})
			return { route, params }
		}
	}
	return null
}

export function route(method: string, path: string, handler: RouteHandler): Route {
	const paramNames: string[] = []
	const pattern = path.replace(/:(\w+)/g, (_match, name) => {
		paramNames.push(name)
		return "([^/]+)"
	})
	return { method, pattern: new RegExp(`^${pattern}$`), paramNames, handler }
}

export function createServer() {
	const state = new AppState()
	const routes: Route[] = [
		...healthRoutes,
		...agentRoutes,
		...swarmRoutes,
		...searchRoutes,
	]

	return createHttpServer(async (req, res) => {
		cors(res)

		if (req.method === "OPTIONS") {
			res.writeHead(204)
			res.end()
			return
		}

		const url = (req.url ?? "/").split("?")[0]

		const matched = matchRoute(routes, req.method ?? "GET", url)
		if (!matched) {
			json(res, 404, { error: "Not found" })
			return
		}

		try {
			const body = req.method === "POST" || req.method === "PUT" ? await parseBody(req) : {}
			await matched.route.handler(req, res, state, body, matched.params)
		} catch (err) {
			const message = err instanceof Error ? err.message : String(err)
			json(res, 500, { error: message })
		}
	})
}

export { json }
