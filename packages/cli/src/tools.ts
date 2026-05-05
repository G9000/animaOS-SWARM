import { action } from "@animaOS-SWARM/core"
import { ALL_TOOL_ACTIONS } from "@animaOS-SWARM/tools"

const extraTools = [
	action({
		name: "get_current_time",
		description: "Get the current date and time",
		parametersSchema: { type: "object", properties: {}, required: [] },
		handler: async () => ({
			status: "success" as const,
			data: new Date().toISOString(),
			durationMs: 0,
		}),
	}),
	action({
		name: "calculate",
		description: "Evaluate a math expression and return the result",
		parametersSchema: {
			type: "object",
			properties: {
				expression: { type: "string", description: "The math expression to evaluate" },
			},
			required: ["expression"],
		},
		handler: async (_runtime, _message, args) => {
			try {
				const expr = args.expression as string
				const result = Function(`"use strict"; return (${expr})`)()
				return { status: "success" as const, data: String(result), durationMs: 0 }
			} catch (err) {
				return { status: "error" as const, error: String(err), durationMs: 0 }
			}
		},
	}),
]

/** All tools available to agents — file tools + web_fetch + calculate + time */
export const allTools = [...ALL_TOOL_ACTIONS, ...extraTools]
