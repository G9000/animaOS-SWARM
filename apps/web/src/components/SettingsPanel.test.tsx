import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import {
  ACCESS_PROFILES,
  toolNamesForProfile,
  type AccessProfile,
} from '../lib/agent-access';
import type { DaemonProvider } from '../lib/daemon-api';
import type { AgentDetail } from '../lib/types';
import { SettingsPanel, type AgentConfigPatch } from './SettingsPanel';

const providers: DaemonProvider[] = [
  {
    id: 'openai',
    label: 'OpenAI',
    requiresKey: true,
    configured: true,
    apiKeyEnvs: ['OPENAI_API_KEY'],
  },
  {
    id: 'anthropic',
    label: 'Anthropic',
    requiresKey: true,
    configured: true,
    apiKeyEnvs: ['ANTHROPIC_API_KEY'],
  },
];

function agent(overrides: Partial<AgentDetail> = {}): AgentDetail {
  return {
    id: 'agent-main',
    name: 'Nova',
    provider: 'openai',
    model: 'gpt-4.1',
    toolNames: toolNamesForProfile('collaborate'),
    created_at_ms: 1,
    status: 'Idle',
    token_usage: {
      prompt_tokens: 1,
      completion_tokens: 2,
      total_tokens: 3,
    },
    system: 'Be precise',
    messages: [],
    ...overrides,
  };
}

function renderPanel(
  currentAgent: AgentDetail,
  overrides: Partial<{
    saving: boolean;
    resetting: boolean;
    error: string | null;
    saveSettings: (patch: AgentConfigPatch) => Promise<boolean>;
    resetAgent: () => void;
    close: () => void;
  }> = {},
) {
  const props = {
    agent: currentAgent,
    providers,
    saving: false,
    resetting: false,
    error: null,
    saveSettings: vi.fn(async () => true),
    resetAgent: vi.fn(),
    close: vi.fn(),
    ...overrides,
  };

  return { ...render(<SettingsPanel {...props} />), props };
}

describe('SettingsPanel access', () => {
  it.each<AccessProfile>(['observe', 'collaborate', 'operate'])(
    'derives and displays the %s profile regardless of tool order',
    (profileName) => {
      renderPanel(
        agent({ toolNames: toolNamesForProfile(profileName).toReversed() }),
      );

      const accessGroup = screen.getByRole('group', { name: 'Access profile' });
      expect(
        within(accessGroup).getByRole('radio', {
          name: new RegExp(`^${ACCESS_PROFILES[profileName].label}`),
        }),
      ).toBeChecked();
      expect(
        within(accessGroup).getByText(ACCESS_PROFILES[profileName].summary),
      ).toBeVisible();
      expect(
        within(accessGroup).getByText(ACCESS_PROFILES[profileName].risk),
      ).toBeVisible();
    },
  );

  it('keeps an unmatched, duplicate, legacy tool set as explicit Custom access when saving other fields', async () => {
    const user = userEvent.setup();
    const customTools = ['read_file', 'legacy_workspace_tool', 'read_file'];
    const saveSettings = vi.fn(async () => true);
    renderPanel(agent({ toolNames: customTools }), { saveSettings });

    expect(screen.getByText('Custom access')).toBeVisible();
    expect(
      screen.getByText(
        'This agent has a custom tool set. Choose a standard profile to replace it when you save.',
      ),
    ).toBeVisible();
    expect(screen.getByText('3 tools configured')).toBeVisible();
    expect(screen.getByText(customTools.join(', '))).toBeVisible();
    for (const radio of screen.getAllByRole('radio')) {
      expect(radio).not.toBeChecked();
    }

    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Nova Prime');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(saveSettings).toHaveBeenCalledWith({ name: 'Nova Prime' });
    expect(saveSettings.mock.calls[0]?.[0]).not.toHaveProperty('tools');
  });

  it('sends an exact ordered profile tool list only after deliberate selection', async () => {
    const user = userEvent.setup();
    const saveSettings = vi.fn(async () => true);
    renderPanel(agent({ toolNames: ['read_file', 'legacy_workspace_tool'] }), {
      saveSettings,
    });

    await user.click(screen.getByRole('radio', { name: /^Operate/ }));
    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Nova Prime');
    const [provider, model] = screen.getAllByRole('combobox');
    await user.selectOptions(provider, 'anthropic');
    await user.selectOptions(model, '__custom__');
    await user.type(
      screen.getByPlaceholderText('model id, e.g. llama3.1'),
      'anthropic/custom-model',
    );
    const system = screen.getByPlaceholderText(
      'Leave empty for the daemon default.',
    );
    await user.clear(system);
    await user.type(system, 'Be concise');
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    expect(saveSettings).toHaveBeenCalledWith({
      name: 'Nova Prime',
      provider: 'anthropic',
      model: 'anthropic/custom-model',
      system: 'Be concise',
      tools: toolNamesForProfile('operate'),
    });
  });

  it('retains the draft and focuses an announced save error', async () => {
    const user = userEvent.setup();
    const currentAgent = agent({
      toolNames: ['read_file', 'legacy_workspace_tool'],
    });
    const saveSettings = vi.fn(async () => false);
    const view = renderPanel(currentAgent, { saveSettings });

    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Unsaved Nova');
    await user.click(screen.getByRole('radio', { name: /^Observe/ }));
    await user.click(screen.getByRole('button', { name: 'Save changes' }));

    view.rerender(<SettingsPanel {...view.props} error="PATCH failed" />);

    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('PATCH failed');
    expect(alert).toHaveAttribute('aria-live', 'assertive');
    expect(alert).toHaveAttribute('id', 'settings-save-error');
    expect(alert).toHaveFocus();
    expect(
      screen.getByRole('button', { name: 'Save changes' }),
    ).toHaveAttribute('aria-describedby', 'settings-save-error');
    expect(screen.getByDisplayValue('Unsaved Nova')).toBeVisible();
    expect(screen.getByRole('radio', { name: /^Observe/ })).toBeChecked();
  });

  it('does not clobber an unsaved draft when a known tool set is reordered', async () => {
    const user = userEvent.setup();
    const currentAgent = agent({
      toolNames: toolNamesForProfile('collaborate'),
    });
    const view = renderPanel(currentAgent);

    const name = screen.getByDisplayValue('Nova');
    await user.clear(name);
    await user.type(name, 'Unsaved Nova');
    view.rerender(
      <SettingsPanel
        {...view.props}
        agent={{
          ...currentAgent,
          toolNames: currentAgent.toolNames.toReversed(),
        }}
      />,
    );

    expect(screen.getByDisplayValue('Unsaved Nova')).toBeVisible();
    expect(screen.getByRole('radio', { name: /^Collaborate/ })).toBeChecked();
  });

  it('preserves save and reset busy states', () => {
    const currentAgent = agent({ name: 'Nova draft' });
    const view = renderPanel(currentAgent, { saving: true });

    expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Reset' })).toBeEnabled();

    view.rerender(<SettingsPanel {...view.props} saving={false} resetting />);

    expect(screen.getByRole('button', { name: 'No changes' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Resetting…' })).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('Resetting agent…');
  });
});
