import {
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from 'react';
import type { Run, RunMode, Session } from '@animaOS-SWARM/sdk';

import { daemon } from '../lib/daemon-api';
import type { Navigate } from '../lib/hash-route';
import { sessionKey } from '../lib/session-groups';
import {
  SKILL_NOT_IN_TELEGRAM_CHAT,
  parseSlashCommand,
  runSlashCommand,
  type SlashCommand,
  type SlashCommandHandlers,
} from '../lib/slash-commands';
import { isOwnerWritten, type HelperTarget } from '../lib/transcript';

/** The chat state a send or command changes. */
export interface CommandChatPatch {
  draft?: string;
  error?: string | null;
  resend?: null;
  delivery?: null;
}

export interface SessionCommandOptions {
  /** The companion: a route to its own sessions names no agent. */
  companionId: string | null;
  /** A message can go now (the companion is ready, the daemon online and
   *  current, nothing resetting); read when it is sent. */
  canSend: () => boolean;
  /** The route's session, or null for a new chat. */
  routeSessionId: string | null;
  /** Its record, once known: a send needs the session's kind. */
  session: Session | null;
  /** The open conversation's chat state key. */
  chatKey: string | null;
  draft: string;
  /** A restored message; sent again unchanged, it keeps its key. */
  resend: { text: string; idempotencyKey: string } | null;
  /** A Telegram session's connector is there to carry replies. */
  telegramReady: boolean;
  /** The open session's reply in progress. */
  activeRun: Run | null;
  updateChat: (key: string, patch: CommandChatPatch) => void;
  /** Starts a new chat with its first message (spec §3.3), sent with
   *  `skill` when it is a `/skill` message. */
  startChat: (text: string, skill?: string) => void;
  queueSend: (
    target: Pick<Session, 'agentId' | 'id' | 'kind'>,
    conversation: string,
    text: string,
    idempotencyKey: string,
    mode?: RunMode,
    skill?: string,
  ) => void;
  /** The commands the composer offers: the built-ins and the skills. */
  slashCommands: readonly SlashCommand[];
  /** Reads the open session's ledger again. */
  refreshRuns: () => void;
  /** Shows (or clears) the composer's error. */
  setError: (message: string | null) => void;
  navigate: Navigate;
  /** The sidebar's sessions, which a fresh record replaces. */
  listedSessions: readonly Session[];
  upsertSession: (session: Session) => void;
  setKnownSession: Dispatch<SetStateAction<Session | null>>;
  newChat: () => void;
  /** Opens the command menu (`/help`). */
  showCommands: () => void;
  search: (words: string) => void;
  /** Opens Settings, where the model is chosen (`/model`). */
  chooseModel: () => void;
  rename: (session: Session, title: string) => Promise<boolean>;
  archive: (session: Session, archived: boolean) => Promise<boolean>;
  exportSession: (session: Session) => Promise<void>;
}

export interface SessionCommands {
  /** Sends the draft, or `text` (a command picked from the menu), as a
   *  queued message; a slash command runs instead. */
  send: (text?: string) => void;
  /** Steers the draft into the reply in progress (⌘/Ctrl+Enter). */
  steer: () => void;
  /** Stops a run (spec §4.6): the reply in progress, or a queued message. */
  stopRun: (run: Pick<Run, 'agentId' | 'id'>) => Promise<void>;
  /** Folds earlier turns into the session summary (spec §5.4). */
  compactSession: (session: Session) => Promise<void>;
  /** Sends a failed or interrupted run's message again, as a new message;
   *  false when it cannot go from the open session or the owner did not
   *  write it. */
  sendAgain: (run: Run) => boolean;
  /** Opens a session, another agent's (a helper's) by its agent. */
  openTarget: (target: HelperTarget) => void;
  /** A manual Compact is in flight (spec §15.2, S3b-C): the trimmed
   *  divider's button disables and shows "Compacting…" meanwhile. */
  compacting: boolean;
}

/**
 * What the composer and the transcript do in the open session (spec §4.2,
 * §4.6, §5.4, §15.3): send, steer, and slash commands, stop, compact, send
 * again, and open another session. The callbacks keep their identity and
 * read the newest options when called.
 */
export function useSessionCommands(
  options: SessionCommandOptions,
): SessionCommands {
  const latest = useRef(options);
  latest.current = options;
  const [compacting, setCompacting] = useState(false);
  const commands = useMemo(() => {
    /** A fresh record replaces the listed or known copy. */
    const adoptSession = (session: Session) => {
      const { listedSessions, upsertSession, setKnownSession } = latest.current;
      const key = sessionKey(session);
      if (listedSessions.some((item) => sessionKey(item) === key))
        upsertSession(session);
      setKnownSession((current) =>
        current && sessionKey(current) === key ? session : current,
      );
    };

    const stopRun = async (run: Pick<Run, 'agentId' | 'id'>) => {
      try {
        await daemon.stopRun(run.agentId, run.id);
        latest.current.refreshRuns();
      } catch (caught) {
        latest.current.setError(errorMessage(caught));
      }
    };

    const compactSession = async (session: Session) => {
      setCompacting(true);
      try {
        // The summary itself arrives with `session.updated`.
        adoptSession(await daemon.compactSession(session.agentId, session.id));
        latest.current.setError(null);
      } catch (caught) {
        latest.current.setError(errorMessage(caught));
      } finally {
        setCompacting(false);
      }
    };

    /** The commands the composer runs itself (spec §15.3). */
    const handlers = (): SlashCommandHandlers => {
      const current = latest.current;
      const result: SlashCommandHandlers = {
        new: () => current.newChat(),
        help: () => current.showCommands(),
        search: (words) => current.search(words),
        model: () => current.chooseModel(),
        usage: () => current.navigate({ kind: 'page', page: 'usage' }),
      };
      const { session, activeRun } = current;
      if (!session) return result;
      const { capabilities } = session;
      if (activeRun && capabilities.stop)
        result.stop = () => void stopRun(activeRun);
      if (capabilities.rename)
        result.rename = (title) => void current.rename(session, title);
      if (capabilities.archive)
        result.archive = () => void current.archive(session, !session.archived);
      if (capabilities.export)
        result.export = () => void current.exportSession(session);
      if (capabilities.compact)
        result.compact = () => void compactSession(session);
      return result;
    };

    const submit = (mode: RunMode, override?: string) => {
      const current = latest.current;
      if (!current.canSend()) return;
      const text = (override ?? current.draft).trim();
      if (!text) return;
      const command = parseSlashCommand(text, current.slashCommands);
      // A skill is a message: the whole text goes, with its skill (spec §15.3).
      const skill = command?.command.skill;
      if (command && !skill && current.chatKey) {
        const key = current.chatKey;
        current.updateChat(key, { draft: '', error: null });
        const problem = runSlashCommand(command, handlers());
        // A command that cannot run here says why and stays in the composer.
        if (problem) current.updateChat(key, { draft: text, error: problem });
        return;
      }
      if (!current.routeSessionId) {
        if (skill) current.startChat(text, skill);
        else current.startChat(text);
        return;
      }
      const { session, activeRun, chatKey } = current;
      // Until its record loads, the session's kind is unknown.
      if (!session || !chatKey) return;
      if (session.kind === 'telegram' && skill) {
        current.updateChat(chatKey, {
          draft: text,
          error: SKILL_NOT_IN_TELEGRAM_CHAT,
        });
        return;
      }
      if (session.kind === 'telegram' && !current.telegramReady) return;
      // A restored message sent unchanged keeps its key; anything else is new.
      const idempotencyKey =
        current.resend?.text === text
          ? current.resend.idempotencyKey
          : crypto.randomUUID();
      current.updateChat(chatKey, {
        draft: '',
        error: null,
        resend: null,
        delivery: null,
      });
      // A skill message never steers (the daemon refuses it).
      const sendMode =
        mode === 'steer' && !skill && activeRun && session.capabilities.steer
          ? 'steer'
          : 'queue';
      if (skill)
        current.queueSend(
          session,
          chatKey,
          text,
          idempotencyKey,
          sendMode,
          skill,
        );
      else current.queueSend(session, chatKey, text, idempotencyKey, sendMode);
    };

    return {
      send: (text?: string) => submit('queue', text),
      steer: () => submit('steer'),
      stopRun,
      compactSession,
      sendAgain: (run: Run) => {
        const { session, chatKey, queueSend } = latest.current;
        if (
          !session ||
          !chatKey ||
          run.agentId !== session.agentId ||
          run.sessionId !== session.id ||
          !isOwnerWritten(run)
        )
          return false;
        const key = crypto.randomUUID();
        if (run.input.skill)
          queueSend(
            session,
            chatKey,
            run.input.text,
            key,
            'queue',
            run.input.skill,
          );
        else queueSend(session, chatKey, run.input.text, key);
        return true;
      },
      openTarget: (target: HelperTarget) =>
        latest.current.navigate({
          kind: 'session',
          sessionId: target.sessionId,
          ...(target.agentId !== latest.current.companionId
            ? { agentId: target.agentId }
            : {}),
        }),
    };
  }, []);
  return { ...commands, compacting };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
