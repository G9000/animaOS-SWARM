-- The runtime's logical step identity is `(agent_id, idempotency_key)` — that's
-- what `tool_step_idempotency_key` derives in `anima-core/src/runtime.rs`.
-- `step_index` is only an ordering hint, but the original schema also gave it
-- a UNIQUE (agent_id, step_index) constraint. That second constraint is what
-- the upsert in `postgres.rs` does NOT handle (`ON CONFLICT` can only target
-- one constraint), so two writes with different idempotency keys but the same
-- step_index — possible across snapshot restores or multi-writer scenarios —
-- would error instead of upserting.
--
-- Drop the unique constraint and replace it with a plain index so
-- `list_agent_steps`'s `ORDER BY step_index` stays cheap.

ALTER TABLE step_log
    DROP CONSTRAINT IF EXISTS step_log_agent_id_step_index_key;

CREATE INDEX IF NOT EXISTS step_log_agent_id_step_index_idx
    ON step_log (agent_id, step_index);
