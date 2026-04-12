CREATE TABLE IF NOT EXISTS step_log (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    step_index      INTEGER NOT NULL,
    idempotency_key TEXT NOT NULL,
    type            TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('pending', 'done', 'failed')),
    input           JSONB,
    output          JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (agent_id, step_index),
    UNIQUE (agent_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS step_log_agent_idx ON step_log (agent_id);
