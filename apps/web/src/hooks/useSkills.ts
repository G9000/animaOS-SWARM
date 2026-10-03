import { useCallback, useEffect, useRef, useState } from 'react';
import {
  DaemonHttpError,
  type Skill,
  type SkillDraft,
  type SkillDraftApproval,
  type SkillInput,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';

export interface SkillsOptions {
  /** `LiveState.skillsVersion`: bumped by every `skill.updated`. */
  version: number;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
  /** False while the daemon is offline: nothing is read. */
  enabled: boolean;
}

export interface SkillsView {
  skills: Skill[];
  /** Waiting for the owner, oldest first, files without a record included. */
  pending: SkillDraft[];
  /** Decided in the last 30 days, newest first. */
  decided: SkillDraft[];
  loaded: boolean;
  error: string | null;
  /** The HTTP status of the failed action, when the daemon refused it. */
  errorStatus: number | null;
  /** Why skills cannot be used at all (no workspace), from the daemon. */
  unavailable: string | null;
  /** Reads the lists again. */
  refresh: () => void;
  /** Each answers true when the daemon took it, after the lists were read
   *  again. */
  save: (slug: string, input: SkillInput) => Promise<boolean>;
  setEnabled: (skill: Skill, enabled: boolean) => Promise<boolean>;
  remove: (skill: Skill) => Promise<boolean>;
  approveChanged: (skill: Skill, hash: string) => Promise<boolean>;
  approveDraft: (
    draft: SkillDraft,
    approval?: SkillDraftApproval,
  ) => Promise<boolean>;
  rejectDraft: (draft: SkillDraft) => Promise<boolean>;
  importFile: (file: File) => Promise<boolean>;
}

function message(error: unknown): string {
  return error instanceof DaemonHttpError
    ? error.message
    : COMPANION_UNREACHABLE;
}

/** The Skills page's data (spec §15.4, §15.5 `useSkills`). */
export function useSkills({
  version,
  epoch,
  enabled,
}: SkillsOptions): SkillsView {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [pending, setPending] = useState<SkillDraft[]>([]);
  const [decided, setDecided] = useState<SkillDraft[]>([]);
  const [loaded, setLoaded] = useState(false);
  // A failed read and a failed action keep their own messages, so a
  // successful reload does not hide a refused action, nor the reverse.
  const [readError, setReadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionStatus, setActionStatus] = useState<number | null>(null);
  const [unavailable, setUnavailable] = useState<string | null>(null);
  const reading = useRef<AbortController | null>(null);

  // Reads the three lists together; a newer read (or unmount) drops this one.
  const load = useCallback(async () => {
    reading.current?.abort();
    const controller = new AbortController();
    reading.current = controller;
    const { signal } = controller;
    try {
      const [nextSkills, nextPending, nextDecided] = await Promise.all([
        daemon.listSkills({ signal }),
        daemon.listSkillDrafts({ status: 'pending', signal }),
        daemon.listSkillDrafts({ status: 'decided', signal }),
      ]);
      if (signal.aborted) return;
      setSkills(nextSkills);
      setPending(nextPending);
      setDecided(nextDecided);
      setUnavailable(null);
      setReadError(null);
      setLoaded(true);
    } catch (caught) {
      if (signal.aborted) return;
      if (caught instanceof DaemonHttpError && caught.status === 409) {
        setUnavailable(caught.message);
        setReadError(null);
      } else {
        setReadError(message(caught));
      }
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    void load();
  }, [enabled, version, epoch, load]);

  useEffect(() => () => reading.current?.abort(), []);

  const refresh = useCallback(() => void load(), [load]);

  // The stream's `skill.updated` would read again too, but the page must
  // not depend on the stream, so an action reads before it answers.
  const act = useCallback(
    async (work: () => Promise<unknown>) => {
      // A new action does not keep showing the last one's refusal.
      setActionError(null);
      setActionStatus(null);
      try {
        await work();
      } catch (caught) {
        setActionError(message(caught));
        if (caught instanceof DaemonHttpError) {
          setActionStatus(caught.status);
          // The thing acted on changed or went away since the lists were
          // read: read them again so the page shows what is there now.
          if (caught.status === 404 || caught.status === 409) await load();
        }
        return false;
      }
      await load();
      return true;
    },
    [load],
  );

  return {
    skills,
    pending,
    decided,
    loaded,
    error: actionError ?? readError,
    errorStatus: actionError ? actionStatus : null,
    unavailable,
    refresh,
    save: (slug, input) => act(() => daemon.saveSkill(slug, input)),
    setEnabled: (skill, value) =>
      act(() => daemon.setSkillEnabled(skill.slug, value)),
    remove: (skill) => act(() => daemon.deleteSkill(skill.slug)),
    approveChanged: (skill, hash) =>
      act(() => daemon.approveSkill(skill.slug, hash)),
    approveDraft: (draft, approval = {}) =>
      act(() => daemon.approveSkillDraft(draft.id, approval)),
    rejectDraft: (draft) => act(() => daemon.rejectSkillDraft(draft.id)),
    importFile: (file) => act(() => daemon.importSkill(file)),
  };
}
