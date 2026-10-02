import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { approvalFixture } from '../../test/live';
import { ApprovalCard } from './ApprovalCard';

describe('ApprovalCard', () => {
  it('shows the call’s arguments as text, never as markup', () => {
    const approval = approvalFixture('apr_1', {
      arguments: '{"command":"<img src=x onerror=alert(1)> **bold**"}',
      argumentsTruncated: true,
    });
    const { container } = render(<ApprovalCard approval={approval} />);

    const card = screen.getByRole('region', { name: 'Approval needed: bash' });
    expect(within(card).getByLabelText('Arguments')).toHaveTextContent(
      '{"command":"<img src=x onerror=alert(1)> **bold**"}',
    );
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('strong')?.textContent).toBe('bash');
    expect(
      within(card).getByText('Arguments shortened to 16 KiB.'),
    ).toBeVisible();
    expect(within(card).getByText('Runs commands')).toBeVisible();
    expect(within(card).getByLabelText('Note (optional)')).toHaveAttribute(
      'maxLength',
      '1000',
    );
    for (const name of [
      'Allow once',
      'Allow for this session',
      'Always allow',
      'Deny',
    ])
      expect(within(card).getByRole('button', { name })).toBeDisabled();
  });

  it('makes hidden characters in the arguments visible', () => {
    const approval = approvalFixture('apr_1', {
      arguments: '{"command":"ls\u202E"}',
    });
    render(<ApprovalCard approval={approval} />);

    expect(screen.getByLabelText('Arguments')).toHaveTextContent(
      '{"command":"ls\\u{202e}"}',
    );
  });

  it('sends a trimmed note with a denial and no matcher', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const onDecide = vi.fn().mockResolvedValue(null);
    render(<ApprovalCard approval={approval} onDecide={onDecide} />);

    await user.type(screen.getByLabelText('Note (optional)'), '  not now  ');
    await user.click(screen.getByRole('button', { name: 'Deny' }));

    expect(onDecide).toHaveBeenCalledWith(approval, {
      decision: 'deny',
      note: 'not now',
    });
    expect(
      await screen.findByText('Denied. The companion carries on without it.'),
    ).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'Allow once' }),
    ).not.toBeInTheDocument();
  });

  it('sends the suggested scope, or the edited one, with the scoped decisions', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const onDecide = vi.fn().mockResolvedValue(null);
    const { unmount } = render(
      <ApprovalCard approval={approval} onDecide={onDecide} />,
    );
    expect(
      screen.getByText(/bash commands starting with “git status”/),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Edit' }));
    const value = screen.getByLabelText('Match value');
    await user.clear(value);
    await user.type(value, 'git');
    await user.click(screen.getByRole('button', { name: 'Always allow' }));
    expect(onDecide).toHaveBeenLastCalledWith(approval, {
      decision: 'allow_always',
      matcher: { kind: 'command_prefix', value: 'git' },
    });
    unmount();

    render(<ApprovalCard approval={approval} onDecide={onDecide} />);
    await user.click(screen.getByRole('button', { name: 'Edit' }));
    await user.selectOptions(
      screen.getByLabelText('Match by'),
      'Any call of this tool',
    );
    expect(screen.queryByLabelText('Match value')).not.toBeInTheDocument();
    await user.click(
      screen.getByRole('button', { name: 'Allow for this session' }),
    );
    expect(onDecide).toHaveBeenLastCalledWith(approval, {
      decision: 'allow_session',
      matcher: { kind: 'any', value: '' },
    });
  });

  it('shows why a decision did not go through and lets the owner try again', async () => {
    const user = userEvent.setup();
    const approval = approvalFixture('apr_1');
    const onDecide = vi
      .fn()
      .mockResolvedValueOnce('This approval was already resolved')
      .mockResolvedValueOnce(null);
    render(<ApprovalCard approval={approval} onDecide={onDecide} />);

    await user.click(screen.getByRole('button', { name: 'Allow once' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'This approval was already resolved',
    );
    await user.click(screen.getByRole('button', { name: 'Allow once' }));
    expect(await screen.findByText('Allowed once. Continuing…')).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('warns beside the scoped buttons when the scope is broad', async () => {
    const user = userEvent.setup();
    const warning =
      'A rule this broad lets your companion run almost anything, including changing its own approval settings.';
    render(
      <ApprovalCard
        approval={approvalFixture('apr_1', {
          suggestedMatcher: { kind: 'command_prefix', value: 'ls -la' },
        })}
        onDecide={vi.fn()}
      />,
    );
    expect(screen.queryByText(warning)).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Edit' }));
    await user.selectOptions(
      screen.getByLabelText('Match by'),
      'Any call of this tool',
    );
    expect(screen.getByText(warning)).toBeVisible();
  });

  it('keeps cut arguments and an empty scope from being saved as a rule', async () => {
    const user = userEvent.setup();
    const onDecide = vi.fn().mockResolvedValue(null);
    const { unmount } = render(
      <ApprovalCard
        approval={approvalFixture('apr_1', { argumentsTruncated: true })}
        onDecide={onDecide}
      />,
    );
    expect(
      screen.getByText('Arguments were cut; only Allow once or Deny.'),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Allow for this session' }),
    ).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Always allow' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Allow once' })).toBeEnabled();
    unmount();

    render(
      <ApprovalCard
        approval={approvalFixture('apr_2', {
          suggestedMatcher: { kind: 'command_prefix', value: '' },
        })}
        onDecide={onDecide}
      />,
    );
    expect(screen.queryByText(/starting with “”/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Always allow' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Edit' }));
    await user.type(screen.getByLabelText('Match value'), 'ls');
    expect(screen.getByRole('button', { name: 'Always allow' })).toBeEnabled();
  });

  it('offers no rule for another agent’s approval', () => {
    render(
      <ApprovalCard
        approval={approvalFixture('apr_1', { agentId: 'helper-7' })}
        onDecide={vi.fn()}
        canPersist={false}
      />,
    );

    expect(
      screen.queryByRole('button', { name: 'Always allow' }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText('Rules for other agents are not managed here yet.'),
    ).toBeVisible();
    for (const name of ['Allow once', 'Allow for this session', 'Deny'])
      expect(screen.getByRole('button', { name })).toBeEnabled();
  });
});
