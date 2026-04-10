import type {
  GenerateOptions,
  GenerateResult,
  IModelAdapter,
  ModelConfig,
} from '@animaOS-SWARM/core';

function readLastUserText(options: GenerateOptions): string {
  const lastMessage = [...options.messages]
    .reverse()
    .find((message) => message.role === 'user');

  return lastMessage?.content.text ?? 'No task provided.';
}

export class MockModelAdapter implements IModelAdapter {
  provider = 'test' as const;

  async generate(
    _config: ModelConfig,
    options: GenerateOptions
  ): Promise<GenerateResult> {
    const task = readLastUserText(options);

    return {
      content: {
        text: `Mock completion for: ${task}`,
      },
      toolCalls: undefined,
      usage: {
        promptTokens: 10,
        completionTokens: 5,
        totalTokens: 15,
      },
      stopReason: 'end',
    };
  }
}
