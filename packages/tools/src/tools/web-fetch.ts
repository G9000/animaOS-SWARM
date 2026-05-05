// @animaOS-SWARM/tools — tools/web-fetch.ts
import type { Action } from "@animaOS-SWARM/core";

export interface WebFetchArgs {
	url: string;
	max_length?: number;
}

export async function executeWebFetch(args: WebFetchArgs): Promise<string> {
	const { url, max_length = 10000 } = args;

	const response = await fetch(url, {
		headers: {
			"User-Agent": "animaOS-SWARM/0.1",
			"Accept": "text/html,application/json,text/plain,*/*",
		},
		signal: AbortSignal.timeout(15000),
	});

	if (!response.ok) {
		throw new Error(`HTTP ${response.status}: ${response.statusText}`);
	}

	const contentType = response.headers.get("content-type") ?? "";
	let text: string;

	if (contentType.includes("application/json")) {
		const json = await response.json();
		text = JSON.stringify(json, null, 2);
	} else {
		text = await response.text();

		// Strip HTML tags for cleaner output
		if (contentType.includes("text/html")) {
			// Remove script and style blocks
			text = text.replace(/<script[\s\S]*?<\/script>/gi, "");
			text = text.replace(/<style[\s\S]*?<\/style>/gi, "");
			// Remove HTML tags
			text = text.replace(/<[^>]+>/g, " ");
			// Collapse whitespace
			text = text.replace(/\s+/g, " ").trim();
		}
	}

	if (text.length > max_length) {
		text = text.slice(0, max_length) + "\n...[truncated]";
	}

	return text;
}

export const webFetchAction: Action = {
	name: "web_fetch",
	description: "Fetch the content of a URL. Returns the text content of the page (HTML tags stripped) or JSON response.",
	parametersSchema: {
		type: "object",
		properties: {
			url: { type: "string", description: "The URL to fetch" },
			max_length: { type: "number", description: "Maximum character length of response (default: 10000)" },
		},
		required: ["url"],
	},
	handler: async (_runtime, _message, args) => {
		try {
			const result = await executeWebFetch(args as unknown as WebFetchArgs);
			return { status: "success" as const, data: result, durationMs: 0 };
		} catch (err) {
			return { status: "error" as const, error: String(err), durationMs: 0 };
		}
	},
};
