import { createRef } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ModelStep, type ModelStepProps } from './ModelStep';

vi.mock('../ChatGptConnection', () => ({
  ChatGptConnection: ({ onConnectionChange }: { onConnectionChange(): void }) => (
    <button onClick={onConnectionChange}>Complete ChatGPT sign-in</button>
  ),
}));

it('lets a subscription-only user connect and select ChatGPT without another provider', () => {
  const props: ModelStepProps = {
    providers: [
      { id: 'chatgpt', label: 'ChatGPT', configured: false, requiresKey: false, apiKeyEnvs: [] },
      { id: 'anthropic', label: 'Anthropic', configured: false, requiresKey: true, apiKeyEnvs: ['ANTHROPIC_API_KEY'] },
    ],
    catalogState: 'empty', providerError: null, provider: '', model: '', customModel: '',
    onProviderChange: vi.fn(), onModelChange: vi.fn(), onCustomModelChange: vi.fn(),
    onRetryProviders: vi.fn(), modelSelectRef: createRef(), customModelInputRef: createRef(),
  };
  const { rerender } = render(<ModelStep {...props} />);
  expect(screen.getByRole('heading', { name: 'Start with your ChatGPT subscription' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Use ChatGPT subscription' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Complete ChatGPT sign-in' }));
  expect(props.onRetryProviders).toHaveBeenCalledOnce();

  const connectedProps: ModelStepProps = {
    ...props, catalogState: 'ready',
    providers: props.providers!.map((item) => ({ ...item, configured: item.id === 'chatgpt' })),
  };
  rerender(<ModelStep {...connectedProps} />);
  fireEvent.click(screen.getByRole('button', { name: 'Use ChatGPT subscription' }));
  expect(props.onProviderChange).toHaveBeenCalledWith('chatgpt');
  rerender(<ModelStep {...connectedProps} provider="chatgpt" model="gpt-5.5" />);
  expect(screen.getByRole('button', { name: 'ChatGPT subscription selected' })).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByRole('combobox', { name: 'Model' })).toHaveValue('gpt-5.5');
  expect(screen.getByText('Other AI providers (optional)').closest('details')).not.toHaveAttribute('open');
});
