import {
	type AgentRunResponse,
	type AgentSnapshot,
	type DaemonClient,
	type DaemonClientOptions,
	DaemonHttpError,
	type SwarmRunResponse,
	createDaemonClient,
} from "@animaOS-SWARM/sdk"

export type CliDaemonClient = DaemonClient

export function createCliDaemonClient(
	options: DaemonClientOptions = {},
): CliDaemonClient {
	return createDaemonClient({
		...options,
		baseUrl: options.baseUrl ?? process.env.ANIMA_DAEMON_URL,
	})
}

export { DaemonHttpError }
export type { AgentRunResponse, AgentSnapshot, DaemonClientOptions, SwarmRunResponse }
