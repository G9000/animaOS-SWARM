import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonHttpError } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import {
  EDIT_NEEDS_REVIEW,
  FILE_PROBLEM_FIX,
  REVIEW_WARNING,
  SOURCE_LABELS,
  invisibleNote,
  skillExistsProblem,
} from '../lib/skills';
import { skillDraftFixture, skillFixture } from '../test/skills';
import { SkillsPage } from './SkillsPage';

function renderPage() {
  const onOpenSession = vi.fn();
  const view = render(
    <SkillsPage version={0} epoch={1} online onOpenSession={onOpenSession} />,
  );
  return { onOpenSession, ...view };
}

beforeEach(() => {
  vi.spyOn(daemon, 'listSkills').mockResolvedValue([
    skillFixture('notes', { name: 'Notes', description: 'Take notes' }),
    skillFixture('plan', { name: 'Plan', status: 'changed' }),
  ]);
  vi.spyOn(daemon, 'listSkillDrafts').mockImplementation(async ({ status }) =>
    status === 'pending'
      ? [skillDraftFixture('skd_1', { slug: 'weekly', name: 'Weekly' })]
      : [
          skillDraftFixture('skd_0', {
            name: 'Old idea',
            status: 'rejected',
            decidedAtMs: 5,
          }),
        ],
  );
  vi.spyOn(daemon, 'skill').mockResolvedValue({
    skill: skillFixture('notes'),
    file: {
      hash: 'b'.repeat(64),
      name: 'Notes',
      description: 'Take notes',
      body: 'Old line\nKept line',
      problem: null,
    },
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SkillsPage', () => {
  it('lists skills with their status and turns one off', async () => {
    const user = userEvent.setup();
    const toggle = vi
      .spyOn(daemon, 'setSkillEnabled')
      .mockResolvedValue(skillFixture('notes', { enabled: false }));
    renderPage();

    const list = await screen.findByRole('region', { name: 'Skills' });
    const notes = within(list).getByRole('listitem', { name: 'Notes' });
    expect(within(notes).getByText('/notes')).toBeVisible();
    expect(within(notes).getByText('Active')).toBeVisible();
    expect(
      within(within(list).getByRole('listitem', { name: 'Plan' })).getByText(
        'Changed on disk — review it',
      ),
    ).toBeVisible();
    await user.click(
      within(notes).getByRole('checkbox', { name: 'Notes is on' }),
    );
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
    expect(toggle).toHaveBeenCalledWith('notes', false);
    expect(
      within(
        screen.getByRole('region', { name: 'Recently decided' }),
      ).getByText('Old idea'),
    ).toBeVisible();
  });

  it('renders untrusted draft text as text', async () => {
    vi.mocked(daemon.listSkillDrafts).mockImplementation(async ({ status }) =>
      status === 'pending'
        ? [
            skillDraftFixture('skd_1', {
              name: '<b>Bold</b>',
              body: '<img src=x onerror="alert(1)"> **not markdown**',
            }),
          ]
        : [],
    );
    const { container } = renderPage();

    const card = await screen.findByRole('region', {
      name: 'Skill draft: <b>Bold</b>',
    });
    expect(
      within(card).getByText('<img src=x onerror="alert(1)"> **not markdown**'),
    ).toBeVisible();
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('b')).toBeNull();
    expect(container.querySelector('strong b, em')).toBeNull();
  });

  it('shows invisible characters as markers and counts them', async () => {
    vi.mocked(daemon.listSkillDrafts).mockImplementation(async ({ status }) =>
      status === 'pending'
        ? [
            skillDraftFixture('skd_1', {
              name: 'Wea\u202Eving',
              body: 'safe\u200Bhidden',
            }),
          ]
        : [],
    );
    renderPage();

    const card = await screen.findByRole('region', {
      name: 'Skill draft: Wea⟨U+202E⟩ving',
    });
    expect(within(card).getByText('Wea⟨U+202E⟩ving')).toBeVisible();
    expect(within(card).getByText('safe⟨U+200B⟩hidden')).toBeVisible();
    expect(within(card).getByText(invisibleNote(2) ?? '')).toBeVisible();
  });

  it('approves, edits, and rejects drafts', async () => {
    const user = userEvent.setup();
    const approve = vi.spyOn(daemon, 'approveSkillDraft').mockResolvedValue({
      skill: skillFixture('weekly'),
      draft: skillDraftFixture('skd_1', { status: 'approved' }),
    });
    const reject = vi
      .spyOn(daemon, 'rejectSkillDraft')
      .mockResolvedValue(skillDraftFixture('skd_1', { status: 'rejected' }));
    const { onOpenSession } = renderPage();
    const card = await screen.findByRole('region', {
      name: 'Skill draft: Weekly',
    });

    await user.click(
      within(card).getByRole('button', { name: 'Open the chat' }),
    );
    expect(onOpenSession).toHaveBeenCalledWith({
      agentId: 'agent-main',
      sessionId: 'chat:1',
      runId: 'run_1',
    });
    await user.click(within(card).getByRole('button', { name: 'Approve' }));
    expect(approve).toHaveBeenLastCalledWith('skd_1', {});
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));

    await user.click(within(card).getByRole('button', { name: 'Edit' }));
    const body = within(card).getByRole('textbox', { name: 'Instructions' });
    await user.clear(body);
    await user.type(body, 'Edited by me.');
    await user.click(
      within(card).getByRole('button', { name: 'Approve edited version' }),
    );
    expect(approve).toHaveBeenLastCalledWith('skd_1', {
      body: 'Edited by me.',
    });
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(3));

    await user.click(within(card).getByRole('button', { name: 'Reject' }));
    expect(reject).toHaveBeenCalledWith('skd_1');
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(4));
    expect(within(card).getByRole('button', { name: 'Reject' })).toBeEnabled();
  });

  it('approves a file draft at the hash it shows and refuses one with a problem', async () => {
    const user = userEvent.setup();
    vi.mocked(daemon.listSkillDrafts).mockImplementation(async ({ status }) =>
      status === 'pending'
        ? [
            skillDraftFixture('file:found', {
              slug: 'found',
              name: 'Found',
              source: 'file',
              proposedBy: null,
              fileHash: 'f'.repeat(64),
            }),
            skillDraftFixture('file:broken', {
              slug: 'broken',
              name: 'broken',
              source: 'file',
              proposedBy: null,
              problem:
                'SKILL.md must start with front matter between --- lines',
            }),
          ]
        : [],
    );
    const approve = vi.spyOn(daemon, 'approveSkillDraft').mockResolvedValue({
      skill: skillFixture('found'),
      draft: skillDraftFixture('file:found', { status: 'approved' }),
    });
    renderPage();

    const found = await screen.findByRole('region', {
      name: 'Skill draft: Found',
    });
    expect(within(found).getByText(SOURCE_LABELS.file)).toBeVisible();
    await user.click(within(found).getByRole('button', { name: 'Approve' }));
    expect(approve).toHaveBeenCalledWith('file:found', {
      hash: 'f'.repeat(64),
    });
    const broken = screen.getByRole('region', { name: 'Skill draft: broken' });
    expect(
      within(broken).getByText(
        'SKILL.md must start with front matter between --- lines',
      ),
    ).toBeVisible();
    expect(
      within(broken).getByRole('button', { name: 'Approve' }),
    ).toBeDisabled();
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
    expect(
      within(found).getByRole('button', { name: 'Approve' }),
    ).toBeEnabled();
  });

  it('lists at most 20 file drafts and counts the rest', async () => {
    vi.mocked(daemon.listSkillDrafts).mockImplementation(async ({ status }) =>
      status === 'pending'
        ? Array.from({ length: 22 }, (_, index) =>
            skillDraftFixture(`file:s${index}`, {
              slug: `s${index}`,
              name: `Found ${index}`,
              source: 'file',
              proposedBy: null,
              fileHash: 'f'.repeat(64),
            }),
          )
        : [],
    );
    renderPage();

    expect(
      await screen.findByText('2 more in the skills folder'),
    ).toBeVisible();
    expect(
      screen.getAllByRole('region', { name: /^Skill draft:/ }),
    ).toHaveLength(20);
  });

  it('shows a draft that replaces a skill as a diff and warns when it is stale', async () => {
    vi.mocked(daemon.listSkillDrafts).mockImplementation(async ({ status }) =>
      status === 'pending'
        ? [
            skillDraftFixture('skd_2', {
              slug: 'notes',
              name: 'Notes v2',
              body: 'New line\nKept line',
              baseHash: 'a'.repeat(64),
              currentHash: 'b'.repeat(64),
              stale: true,
            }),
          ]
        : [],
    );
    renderPage();

    const card = await screen.findByRole('region', {
      name: 'Skill draft: Notes v2',
    });
    const diff = await within(card).findByLabelText(
      'Changes from the current version',
    );
    expect(diff).toHaveTextContent('- Old line');
    expect(diff).toHaveTextContent('+ New line');
    expect(diff).toHaveTextContent('Kept line');
    expect(within(card).getByRole('note')).toHaveTextContent(
      'This skill changed since the draft was made',
    );
  });

  it('creates a skill with the editor after checking it', async () => {
    const user = userEvent.setup();
    const save = vi
      .spyOn(daemon, 'saveSkill')
      .mockResolvedValue(skillFixture('weekly-review'));
    renderPage();
    await screen.findByRole('region', { name: 'Skills' });

    await user.click(screen.getByRole('button', { name: 'New skill' }));
    const form = screen.getByRole('form', { name: 'Skill editor' });
    await user.type(
      within(form).getByRole('textbox', { name: 'Name' }),
      'Weekly Review',
    );
    expect(
      within(form).getByRole('textbox', { name: 'Folder name' }),
    ).toHaveValue('weekly-review');
    await user.click(within(form).getByRole('button', { name: 'Save skill' }));
    expect(within(form).getByRole('alert')).toHaveTextContent(
      'Say when to use it in one line',
    );
    expect(save).not.toHaveBeenCalled();

    await user.type(
      within(form).getByRole('textbox', { name: 'When to use it' }),
      'Fridays',
    );
    await user.type(
      within(form).getByRole('textbox', { name: 'Instructions' }),
      'List <what> shipped.',
    );
    await user.click(within(form).getByRole('button', { name: 'Preview' }));
    expect(within(form).getByLabelText('Preview')).toHaveTextContent(
      'List <what> shipped.',
    );
    await user.click(within(form).getByRole('button', { name: 'Save skill' }));
    expect(save).toHaveBeenCalledWith('weekly-review', {
      name: 'Weekly Review',
      description: 'Fridays',
      body: 'List <what> shipped.',
    });
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(
        screen.queryByRole('form', { name: 'Skill editor' }),
      ).not.toBeInTheDocument(),
    );
  });

  it('points to the draft found in the folder when a save is refused while it waits', async () => {
    const user = userEvent.setup();
    vi.mocked(daemon.listSkillDrafts).mockImplementation(async ({ status }) =>
      status === 'pending'
        ? [
            skillDraftFixture('file:hi', {
              slug: 'hi',
              name: 'Hi',
              source: 'file',
              proposedBy: null,
              fileHash: 'f'.repeat(64),
            }),
          ]
        : [],
    );
    const save = vi
      .spyOn(daemon, 'saveSkill')
      .mockRejectedValue(
        new DaemonHttpError(409, {
          error: 'The daemon words this its own way',
        }),
      );
    renderPage();
    await screen.findByRole('region', { name: 'Skills' });

    await user.click(screen.getByRole('button', { name: 'New skill' }));
    const form = screen.getByRole('form', { name: 'Skill editor' });
    await user.type(within(form).getByRole('textbox', { name: 'Name' }), 'Hi');
    await user.type(
      within(form).getByRole('textbox', { name: 'When to use it' }),
      'Greeting',
    );
    await user.type(
      within(form).getByRole('textbox', { name: 'Instructions' }),
      'Say hi.',
    );
    await user.click(within(form).getByRole('button', { name: 'Save skill' }));

    expect(save).toHaveBeenCalled();
    expect(await screen.findByText(/its own way/)).toBeVisible();
    expect(
      screen.getByText(/Waiting for review.*found in the skills folder/),
    ).toBeVisible();
  });

  it('reviews a changed skill and approves the version it shows', async () => {
    const user = userEvent.setup();
    const approve = vi
      .spyOn(daemon, 'approveSkill')
      .mockResolvedValue(skillFixture('plan'));
    renderPage();
    const plan = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Plan' });

    await user.click(
      within(plan).getByRole('button', { name: 'Review changes' }),
    );
    const review = await screen.findByRole('region', {
      name: 'Review changes to Plan',
    });
    expect(within(review).getByText(REVIEW_WARNING)).toBeVisible();
    expect(within(review).getByText(/Old line/)).toBeVisible();
    await user.click(
      within(review).getByRole('button', { name: 'Approve this version' }),
    );
    expect(approve).toHaveBeenCalledWith('plan', 'b'.repeat(64));
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(
        screen.queryByRole('region', { name: 'Review changes to Plan' }),
      ).not.toBeInTheDocument(),
    );
  });

  it("a changed skill's Edit opens the review first", async () => {
    const user = userEvent.setup();
    const save = vi.spyOn(daemon, 'saveSkill');
    renderPage();
    const plan = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Plan' });

    await user.click(within(plan).getByRole('button', { name: 'Edit' }));

    const review = await screen.findByRole('region', {
      name: 'Review changes to Plan',
    });
    expect(within(review).getByText(EDIT_NEEDS_REVIEW)).toBeVisible();
    expect(
      screen.queryByRole('form', { name: 'Skill editor' }),
    ).not.toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();
  });

  it('edits an active skill in the editor', async () => {
    const user = userEvent.setup();
    vi.mocked(daemon.skill).mockResolvedValue({
      skill: skillFixture('notes'),
      file: {
        hash: 'a'.repeat(64),
        name: 'Notes',
        description: 'Take notes',
        body: 'Approved body',
        problem: null,
      },
    });
    renderPage();
    const notes = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Notes' });

    await user.click(within(notes).getByRole('button', { name: 'Edit' }));

    const form = await screen.findByRole('form', { name: 'Skill editor' });
    expect(
      within(form).getByRole('textbox', { name: 'Instructions' }),
    ).toHaveValue('Approved body');
    expect(
      screen.queryByRole('region', { name: /^Review changes/ }),
    ).not.toBeInTheDocument();
  });

  it('does not point to a draft when no file draft waits for that folder', async () => {
    const user = userEvent.setup();
    vi.spyOn(daemon, 'saveSkill').mockRejectedValue(
      new DaemonHttpError(409, {
        error:
          "A SKILL.md the owner hasn't reviewed is in this folder; review it first",
      }),
    );
    renderPage();
    await screen.findByRole('region', { name: 'Skills' });

    await user.click(screen.getByRole('button', { name: 'New skill' }));
    const form = screen.getByRole('form', { name: 'Skill editor' });
    await user.type(within(form).getByRole('textbox', { name: 'Name' }), 'Hi');
    await user.type(
      within(form).getByRole('textbox', { name: 'When to use it' }),
      'Greeting',
    );
    await user.type(
      within(form).getByRole('textbox', { name: 'Instructions' }),
      'Say hi.',
    );
    await user.click(within(form).getByRole('button', { name: 'Save skill' }));

    expect(await screen.findByText(/review it first/)).toBeVisible();
    expect(screen.queryByText(/found in the skills folder/)).toBeNull();
  });

  it('refuses a new skill whose folder already holds a skill', async () => {
    const user = userEvent.setup();
    const save = vi.spyOn(daemon, 'saveSkill');
    renderPage();
    await screen.findByRole('region', { name: 'Skills' });

    await user.click(screen.getByRole('button', { name: 'New skill' }));
    const form = screen.getByRole('form', { name: 'Skill editor' });
    await user.type(
      within(form).getByRole('textbox', { name: 'Name' }),
      'Notes',
    );
    await user.type(
      within(form).getByRole('textbox', { name: 'When to use it' }),
      'Again',
    );
    await user.type(
      within(form).getByRole('textbox', { name: 'Instructions' }),
      'Overwrite.',
    );
    await user.click(within(form).getByRole('button', { name: 'Save skill' }));

    expect(within(form).getByRole('alert')).toHaveTextContent(
      'A skill in /notes already exists; edit it instead.',
    );
    expect(skillExistsProblem('notes')).toBe(
      'A skill in /notes already exists; edit it instead.',
    );
    expect(save).not.toHaveBeenCalled();
  });

  it('reveals invisible characters in the editor preview', async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole('region', { name: 'Skills' });

    await user.click(screen.getByRole('button', { name: 'New skill' }));
    const form = screen.getByRole('form', { name: 'Skill editor' });
    fireEvent.change(
      within(form).getByRole('textbox', { name: 'Instructions' }),
      { target: { value: `safe${String.fromCharCode(0x200b)}hidden` } },
    );
    await user.click(within(form).getByRole('button', { name: 'Preview' }));

    expect(within(form).getByLabelText('Preview')).toHaveTextContent(
      'safe⟨U+200B⟩hidden',
    );
    expect(within(form).getByText(invisibleNote(1) ?? '')).toBeVisible();
  });

  it('reads again and shows the new body when approving a file draft is refused', async () => {
    const user = userEvent.setup();
    let body = 'Old body';
    vi.mocked(daemon.listSkillDrafts).mockImplementation(async ({ status }) =>
      status === 'pending'
        ? [
            skillDraftFixture('file:found', {
              slug: 'found',
              name: 'Found',
              source: 'file',
              proposedBy: null,
              fileHash: 'f'.repeat(64),
              body,
            }),
          ]
        : [],
    );
    vi.spyOn(daemon, 'approveSkillDraft').mockImplementation(async () => {
      body = 'New body';
      throw new DaemonHttpError(409, { error: 'The file changed on disk' });
    });
    renderPage();
    const card = await screen.findByRole('region', {
      name: 'Skill draft: Found',
    });
    expect(within(card).getByText('Old body')).toBeVisible();

    await user.click(within(card).getByRole('button', { name: 'Approve' }));

    expect(await screen.findByText('The file changed on disk')).toBeVisible();
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
    expect(await screen.findByText('New body')).toBeVisible();
  });

  it('shows the new file when approving a changed skill is refused', async () => {
    const user = userEvent.setup();
    const detail = vi.mocked(daemon.skill);
    detail.mockResolvedValue({
      skill: skillFixture('plan', { status: 'changed' }),
      file: {
        hash: 'b'.repeat(64),
        name: 'Plan',
        description: 'd',
        body: 'First version',
        problem: null,
      },
    });
    const approve = vi
      .spyOn(daemon, 'approveSkill')
      .mockImplementation(async () => {
        detail.mockResolvedValue({
          skill: skillFixture('plan', { status: 'changed' }),
          file: {
            hash: 'c'.repeat(64),
            name: 'Plan',
            description: 'd',
            body: 'Second version',
            problem: null,
          },
        });
        throw new DaemonHttpError(409, { error: 'The file changed' });
      });
    renderPage();
    const plan = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Plan' });
    await user.click(
      within(plan).getByRole('button', { name: 'Review changes' }),
    );
    const review = await screen.findByRole('region', {
      name: 'Review changes to Plan',
    });
    expect(within(review).getByText('First version')).toBeVisible();

    await user.click(
      within(review).getByRole('button', { name: 'Approve this version' }),
    );

    expect(await within(review).findByText('Second version')).toBeVisible();
    expect(approve).toHaveBeenCalledWith('plan', 'b'.repeat(64));
    expect(screen.getByText('The file changed')).toBeVisible();
    await user.click(
      within(review).getByRole('button', { name: 'Approve this version' }),
    );
    expect(approve).toHaveBeenLastCalledWith('plan', 'c'.repeat(64));
  });

  it('opens the editor when the detail shows the file at the approved hash', async () => {
    const user = userEvent.setup();
    // The list is stale: the detail's own skill record has the new hash.
    vi.mocked(daemon.skill).mockResolvedValue({
      skill: skillFixture('notes', { approvedHash: 'b'.repeat(64) }),
      file: {
        hash: 'b'.repeat(64),
        name: 'Notes',
        description: 'Take notes',
        body: 'Fresh body',
        problem: null,
      },
    });
    renderPage();
    const notes = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Notes' });

    await user.click(within(notes).getByRole('button', { name: 'Edit' }));

    expect(
      await screen.findByRole('form', { name: 'Skill editor' }),
    ).toBeVisible();
  });

  it("shows the daemon's reason when a skill cannot be read, then clears it", async () => {
    const user = userEvent.setup();
    vi.mocked(daemon.skill).mockRejectedValue(
      new DaemonHttpError(404, { error: 'No skill in /notes' }),
    );
    renderPage();
    const notes = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Notes' });

    await user.click(within(notes).getByRole('button', { name: 'Edit' }));
    expect(await screen.findByText('No skill in /notes')).toBeVisible();

    vi.mocked(daemon.skill).mockResolvedValue({
      skill: skillFixture('notes'),
      file: null,
    });
    await user.click(within(notes).getByRole('button', { name: 'Edit' }));
    await waitFor(() =>
      expect(screen.queryByText('No skill in /notes')).toBeNull(),
    );
  });

  it('tells the owner how to fix a file with a problem', async () => {
    const user = userEvent.setup();
    vi.mocked(daemon.skill).mockResolvedValue({
      skill: skillFixture('plan', { status: 'invalid' }),
      file: {
        hash: null,
        name: null,
        description: null,
        body: null,
        problem: 'SKILL.md must start with front matter between --- lines',
      },
    });
    renderPage();
    const plan = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Plan' });

    await user.click(within(plan).getByRole('button', { name: 'Edit' }));

    const review = await screen.findByRole('region', {
      name: 'Review changes to Plan',
    });
    expect(within(review).getByText(FILE_PROBLEM_FIX)).toBeVisible();
  });

  it('deletes a skill only after confirming', async () => {
    const user = userEvent.setup();
    const remove = vi
      .spyOn(daemon, 'deleteSkill')
      .mockResolvedValue({ trashPath: '.anima-trash/skills/notes-1' });
    renderPage();
    const notes = within(
      await screen.findByRole('region', { name: 'Skills' }),
    ).getByRole('listitem', { name: 'Notes' });

    await user.click(within(notes).getByRole('button', { name: 'Delete' }));
    expect(remove).not.toHaveBeenCalled();
    expect(
      within(notes).getByText(/moves to the workspace trash/),
    ).toBeVisible();
    await user.click(
      within(notes).getByRole('button', { name: 'Delete /notes' }),
    );
    expect(remove).toHaveBeenCalledWith('notes');
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(
        within(notes).queryByText(/moves to the workspace trash/),
      ).not.toBeInTheDocument(),
    );
  });

  it('imports a SKILL.md as a draft', async () => {
    const user = userEvent.setup();
    const imported = vi
      .spyOn(daemon, 'importSkill')
      .mockResolvedValue(skillDraftFixture('skd_9', { source: 'import' }));
    renderPage();
    await screen.findByRole('region', { name: 'Skills' });
    const file = new File(['---\nname: n\n---\n\nb'], 'SKILL.md', {
      type: 'text/markdown',
    });

    await user.upload(screen.getByLabelText('Import SKILL.md'), file);

    await waitFor(() => expect(imported).toHaveBeenCalledWith(file));
    await waitFor(() => expect(daemon.listSkills).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('says why skills are unavailable without a workspace', async () => {
    vi.mocked(daemon.listSkills).mockRejectedValue(
      new DaemonHttpError(409, { error: 'Skills need a configured workspace' }),
    );
    renderPage();

    expect(
      await screen.findByText('Skills need a configured workspace'),
    ).toBeVisible();
    expect(
      screen.queryByRole('button', { name: 'New skill' }),
    ).not.toBeInTheDocument();
  });
});
