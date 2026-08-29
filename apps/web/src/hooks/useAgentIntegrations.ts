import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';

import {
  daemon,
  type ConnectorMessage,
  type DaemonSchedule,
  type LegacyScheduleInput,
  type ScheduleCreateInput,
  type TelegramConnector,
} from '../lib/daemon-api';
import {
  createTelegramIdempotencyKey,
  safeIntegrationError,
} from '../lib/telegram';

type ConnectorBusy =
  | 'connect'
  | 'replace'
  | 'approve'
  | 'restart'
  | 'disconnect'
  | 'send'
  | 'messages'
  | null;

export function useAgentIntegrations(agentId: string | null) {
  const generation = useRef(0);
  const connectorMutation = useRef(0);
  const scheduleMutation = useRef(0);
  const connectorBusyRef = useRef(false);
  const scheduleBusyRef = useRef(false);
  const [owner, setOwner] = useState<string | null>(null);
  const [connectors, setConnectors] = useState<TelegramConnector[]>([]);
  const [schedules, setSchedules] = useState<DaemonSchedule[]>([]);
  const [messages, setMessages] = useState<ConnectorMessage[]>([]);
  const [nextBefore, setNextBefore] = useState<string | null>(null);
  const [loading, setLoading] = useState(Boolean(agentId));
  const [connectorBusy, setConnectorBusy] = useState<ConnectorBusy>(null);
  const [scheduleBusy, setScheduleBusy] = useState(false);
  const [connectorError, setConnectorError] = useState<string | null>(null);
  const [scheduleError, setScheduleError] = useState<string | null>(null);
  const [deliveryQueued, setDeliveryQueued] = useState(false);

  useLayoutEffect(() => {
    generation.current += 1;
    connectorMutation.current += 1;
    scheduleMutation.current += 1;
    setOwner(agentId);
    setConnectors([]);
    setSchedules([]);
    setMessages([]);
    setNextBefore(null);
    setConnectorBusy(null);
    setScheduleBusy(false);
    connectorBusyRef.current = false;
    scheduleBusyRef.current = false;
    setConnectorError(null);
    setScheduleError(null);
    setDeliveryQueued(false);
    setLoading(Boolean(agentId));
  }, [agentId]);

  const refresh = useCallback(async () => {
    if (!agentId) return;
    const requestGeneration = generation.current;
    const connectorRequest = connectorMutation.current;
    const scheduleRequest = scheduleMutation.current;
    setLoading(true);
    const [connectorResult, scheduleResult] = await Promise.allSettled([
      daemon.listConnectors(agentId),
      daemon.listSchedules(agentId),
    ]);
    if (requestGeneration !== generation.current) return;
    if (connectorRequest !== connectorMutation.current) {
      // A newer connector mutation owns this state.
    } else if (connectorResult.status === 'fulfilled') {
      setConnectors(connectorResult.value.connectors);
      setConnectorError(null);
    } else {
      setConnectorError(safeIntegrationError(connectorResult.reason));
    }
    if (scheduleRequest !== scheduleMutation.current) {
      // A newer schedule mutation owns this state.
    } else if (scheduleResult.status === 'fulfilled') {
      setSchedules(scheduleResult.value.schedules);
      setScheduleError(null);
    } else {
      setScheduleError(
        scheduleResult.reason instanceof Error
          ? scheduleResult.reason.message
          : 'Schedules could not be loaded.',
      );
    }
    setLoading(false);
  }, [agentId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const connectorAction = useCallback(
    async <T>(
      kind: Exclude<ConnectorBusy, null>,
      action: () => Promise<T>,
      apply: (value: T) => void,
    ) => {
      if (!agentId || connectorBusyRef.current) return false;
      const lifetime = generation.current;
      const mutation = ++connectorMutation.current;
      setConnectorBusy(kind);
      connectorBusyRef.current = true;
      setConnectorError(null);
      try {
        const value = await action();
        if (
          lifetime !== generation.current ||
          mutation !== connectorMutation.current
        )
          return false;
        apply(value);
        return true;
      } catch (error) {
        if (
          lifetime === generation.current &&
          mutation === connectorMutation.current
        ) {
          setConnectorError(safeIntegrationError(error));
        }
        return false;
      } finally {
        if (
          lifetime === generation.current &&
          mutation === connectorMutation.current
        ) {
          connectorBusyRef.current = false;
          setConnectorBusy(null);
        }
      }
    },
    [agentId],
  );

  const upsertConnector = (connector: TelegramConnector) =>
    setConnectors((current) => [
      ...current.filter((item) => item.id !== connector.id),
      connector,
    ]);

  const connectTelegram = useCallback(
    (token: string) =>
      connectorAction(
        'connect',
        () => daemon.createTelegramConnector(agentId!, token),
        ({ connector }) => upsertConnector(connector),
      ),
    [agentId, connectorAction],
  );
  const replaceTelegram = useCallback(
    (connectorId: string, token: string) =>
      connectorAction(
        'replace',
        () => daemon.replaceTelegramCredential(agentId!, connectorId, token),
        ({ connector }) => upsertConnector(connector),
      ),
    [agentId, connectorAction],
  );
  const approvePairing = useCallback(
    (connectorId: string, chatId: string) =>
      connectorAction(
        'approve',
        () => daemon.approveTelegramPairing(agentId!, connectorId, chatId),
        ({ connector }) => upsertConnector(connector),
      ),
    [agentId, connectorAction],
  );
  const restartTelegram = useCallback(
    (connectorId: string) =>
      connectorAction(
        'restart',
        () => daemon.restartTelegramConnector(agentId!, connectorId),
        ({ connector }) => upsertConnector(connector),
      ),
    [agentId, connectorAction],
  );
  const disconnectTelegram = useCallback(
    (connectorId: string) =>
      connectorAction(
        'disconnect',
        () => daemon.deleteTelegramConnector(agentId!, connectorId),
        () => {
          setConnectors((current) =>
            current.filter((item) => item.id !== connectorId),
          );
          setMessages([]);
          setNextBefore(null);
        },
      ),
    [agentId, connectorAction],
  );

  const loadMessages = useCallback(
    async (connectorId: string, older = false) => {
      if (!agentId || connectorBusy) return false;
      const before = older ? (nextBefore ?? undefined) : undefined;
      return connectorAction(
        'messages',
        () =>
          daemon.listConnectorMessages(agentId, connectorId, {
            before,
            limit: 50,
          }),
        (page) => {
          setMessages((current) =>
            older ? [...page.messages, ...current] : page.messages,
          );
          setNextBefore(page.nextBefore);
        },
      );
    },
    [agentId, connectorBusy, connectorAction, nextBefore],
  );

  const sendTelegramMessage = useCallback(
    async (connectorId: string, text: string) =>
      connectorAction(
        'send',
        () =>
          daemon.sendConnectorMessage(
            agentId!,
            connectorId,
            text,
            createTelegramIdempotencyKey(),
          ),
        (response) => {
          setMessages(response.messages);
          setDeliveryQueued(response.deliveryQueued);
        },
      ),
    [agentId, connectorAction],
  );

  const scheduleAction = useCallback(
    async <T>(action: () => Promise<T>, apply: (value: T) => void) => {
      if (!agentId || scheduleBusyRef.current) return false;
      const lifetime = generation.current;
      const mutation = ++scheduleMutation.current;
      setScheduleBusy(true);
      scheduleBusyRef.current = true;
      setScheduleError(null);
      try {
        const value = await action();
        if (
          lifetime !== generation.current ||
          mutation !== scheduleMutation.current
        )
          return false;
        apply(value);
        return true;
      } catch (error) {
        if (
          lifetime === generation.current &&
          mutation === scheduleMutation.current
        ) {
          setScheduleError(
            error instanceof Error ? error.message : 'Schedule request failed.',
          );
        }
        return false;
      } finally {
        if (
          lifetime === generation.current &&
          mutation === scheduleMutation.current
        ) {
          scheduleBusyRef.current = false;
          setScheduleBusy(false);
        }
      }
    },
    [agentId],
  );

  const createSchedule = useCallback(
    (input: ScheduleCreateInput) =>
      scheduleAction(
        () => daemon.createSchedule(agentId!, input),
        ({ schedule }) => setSchedules((current) => [...current, schedule]),
      ),
    [agentId, scheduleAction],
  );
  const removeSchedule = useCallback(
    (id: string) =>
      scheduleAction(
        () => daemon.deleteSchedule(agentId!, id),
        () =>
          setSchedules((current) => current.filter((item) => item.id !== id)),
      ),
    [agentId, scheduleAction],
  );
  const importLegacy = useCallback(
    (items: LegacyScheduleInput[]) =>
      scheduleAction(
        () => daemon.importLegacySchedules(agentId!, { schedules: items }),
        ({ schedules: imported }) => {
          setSchedules((current) => {
            const ids = new Set(imported.map((item) => item.id));
            return [
              ...current.filter((item) => !ids.has(item.id)),
              ...imported,
            ];
          });
        },
      ),
    [agentId, scheduleAction],
  );

  const visible = owner === agentId;
  return {
    connectors: visible ? connectors : [],
    schedules: visible ? schedules : [],
    messages: visible ? messages : [],
    nextBefore: visible ? nextBefore : null,
    loading,
    connectorBusy,
    scheduleBusy,
    connectorError,
    scheduleError,
    deliveryQueued,
    refresh,
    connectTelegram,
    replaceTelegram,
    approvePairing,
    restartTelegram,
    disconnectTelegram,
    loadMessages,
    sendTelegramMessage,
    createSchedule,
    removeSchedule,
    importLegacy,
  };
}
