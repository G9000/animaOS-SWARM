-- History store (companion console spec §13.1): committed session messages,
-- finished runs, and the tables later milestones fill (usage, approvals,
-- automation runs, attachments). The daemon's outbox writes every row
-- idempotently by id; `record` holds the full JSON record.

CREATE TABLE IF NOT EXISTS history_messages (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    role          TEXT NOT NULL,
    text          TEXT NOT NULL,
    hidden        BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms BIGINT NOT NULL,
    ordinal       BIGINT NOT NULL,
    record        JSONB NOT NULL,
    search        tsvector GENERATED ALWAYS AS (to_tsvector('simple', text)) STORED
);
CREATE INDEX IF NOT EXISTS history_messages_session_order_idx
    ON history_messages (agent_id, session_id, created_at_ms, ordinal, id);
CREATE INDEX IF NOT EXISTS history_messages_search_idx
    ON history_messages USING GIN (search);

CREATE TABLE IF NOT EXISTS history_runs (
    id             TEXT PRIMARY KEY,
    agent_id       TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    status         TEXT NOT NULL,
    created_at_ms  BIGINT NOT NULL,
    finished_at_ms BIGINT,
    record         JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_runs_session_idx
    ON history_runs (agent_id, session_id, created_at_ms);

CREATE TABLE IF NOT EXISTS history_usage (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT,
    run_id        TEXT,
    created_at_ms BIGINT NOT NULL,
    record        JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_usage_created_idx ON history_usage (created_at_ms);

CREATE TABLE IF NOT EXISTS history_approvals (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT,
    created_at_ms BIGINT NOT NULL,
    record        JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_approvals_created_idx
    ON history_approvals (agent_id, created_at_ms);

CREATE TABLE IF NOT EXISTS history_schedule_runs (
    id          TEXT PRIMARY KEY,
    schedule_id TEXT NOT NULL,
    agent_id    TEXT NOT NULL,
    fired_at_ms BIGINT NOT NULL,
    record      JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_schedule_runs_schedule_idx
    ON history_schedule_runs (schedule_id, fired_at_ms);

CREATE TABLE IF NOT EXISTS history_attachments (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    created_at_ms BIGINT NOT NULL,
    record        JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS history_attachments_session_idx
    ON history_attachments (agent_id, session_id);
