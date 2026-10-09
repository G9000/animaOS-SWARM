import { useCallback, useEffect, useRef, useState } from 'react';
import {
  DaemonHttpError,
  type Automation,
  type AutomationInput,
  type AutomationPatch,
} from '@animaOS-SWARM/sdk';

import { COMPANION_UNREACHABLE } from '../lib/approvals';
import { daemon } from '../lib/daemon-api';

export interface AutomationsOptions {
  /** The companion whose automations these are; null reads nothing. */
  agentId: string | null;
  /** `LiveState.automationsVersion`: bumped by every `automation.updated`. */
  version: number;
  /** `LiveState.epoch`: bumped by every snapshot and resync. */
  epoch: number;
  /** False while the daemon is offline: nothing is read. */
  enabled: boolean;
}

export interface AutomationsView {
  automations: Automation[];
  loaded: boolean;
  error: string | null;
  /** The HTTP status of the failed action, when the daemon refused it. */
  errorStatus: number | null;
  refresh: () => void;
  /** Each answers true when the daemon took it, after the list was read
   *  again. */
  create: (input: AutomationInput) => Promise<boolean>;
  createHeartbeat: (timeZone: string) => Promise<boolean>;
  update: (automation: Automation, patch: AutomationPatch) => Promise<boolean>;
  setEnabled: (automation: Automation, enabled: boolean) => Promise<boolean>;
  remove: (automation: Automation) => Promise<boolean>;
  /** Deletes like `remove`, but a refusal rejects with the daemon's error
   *  instead of landing in `error`: the notice card's Undo shows it. */
  undo: (automation: Automation) => Promise<boolean>;
  runNow: (automation: Automation) => Promise<boolean>;
}

function message(error: unknown): string {
  return error instanceof DaemonHttpError
    ? error.message
    : COMPANION_UNREACHABLE;
}

/** The companion's automations (spec §15.4, §15.5 `useAutomations`). */
export function useAutomations({
  agentId,
  version,
  epoch,
  enabled,
}: AutomationsOptions): AutomationsView {
  const [automations, setAutomations] = useState<Automation[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [readError, setReadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionStatus, setActionStatus] = useState<number | null>(null);
  const reading = useRef<AbortController | null>(null);

  const load = useCallback(async () => {
    if (!agentId) return;
    reading.current?.abort();
    const controller = new AbortController();
    reading.current = controller;
    try {
      const next = await daemon.listAutomations(agentId, {
        signal: controller.signal,
      });
      if (controller.signal.aborted) return;
      // An empty reload keeps the empty list it had, so a harness that goes
      // online with no automations renders nothing more (the M5 I4 lesson).
      setAutomations((previous) =>
        previous.length === 0 && next.length === 0 ? previous : next,
      );
      setReadError(null);
      setLoaded(true);
    } catch (caught) {
      if (controller.signal.aborted) return;
      setReadError(message(caught));
      setLoaded(true);
    }
  }, [agentId]);

  useEffect(() => {
    if (!enabled || !agentId) return;
    void load();
  }, [enabled, agentId, version, epoch, load]);

  useEffect(() => () => reading.current?.abort(), []);

  const refresh = useCallback(() => void load(), [load]);

  const act = useCallback(
    async (work: (agentId: string) => Promise<unknown>, rethrow = false) => {
      if (!agentId) return false;
      setActionError(null);
      setActionStatus(null);
      try {
        await work(agentId);
      } catch (caught) {
        if (rethrow) {
          if (
            caught instanceof DaemonHttpError &&
            (caught.status === 404 || caught.status === 409)
          )
            await load();
          throw caught;
        }
        setActionError(message(caught));
        if (caught instanceof DaemonHttpError) {
          setActionStatus(caught.status);
          if (caught.status === 404 || caught.status === 409) await load();
        }
        return false;
      }
      await load();
      return true;
    },
    [agentId, load],
  );

  const create = useCallback(
    (input: AutomationInput) => act((id) => daemon.createAutomation(id, input)),
    [act],
  );
  const createHeartbeat = useCallback(
    (timeZone: string) => act((id) => daemon.createHeartbeat(id, { timeZone })),
    [act],
  );
  const update = useCallback(
    (automation: Automation, patch: AutomationPatch) =>
      act((id) => daemon.updateAutomation(id, automation.id, patch)),
    [act],
  );
  const setEnabled = useCallback(
    (automation: Automation, value: boolean) =>
      act((id) =>
        daemon.updateAutomation(id, automation.id, { enabled: value }),
      ),
    [act],
  );
  const remove = useCallback(
    (automation: Automation) =>
      act((id) => daemon.deleteAutomation(id, automation.id)),
    [act],
  );
  const undo = useCallback(
    (automation: Automation) =>
      act((id) => daemon.deleteAutomation(id, automation.id), true),
    [act],
  );
  const runNow = useCallback(
    (automation: Automation) =>
      act((id) => daemon.runAutomationNow(id, automation.id)),
    [act],
  );

  return {
    automations,
    loaded,
    error: actionError ?? readError,
    errorStatus: actionError ? actionStatus : null,
    refresh,
    create,
    createHeartbeat,
    update,
    setEnabled,
    remove,
    undo,
    runNow,
  };
}
