export interface ModToolHandler {
  name: string;
  description: string;
  parameters: {
    type: 'object';
    properties: Record<string, unknown>;
    required?: string[];
  };
  execute(args: Record<string, unknown>): Promise<unknown>;
}

export interface ModPlugin {
  name: string;
  description: string;
  tools: ModToolHandler[];
}
