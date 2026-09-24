-- Keep completed proofs and the exact signed Ethereum transaction across
-- transport failures and worker restarts. A hash is recorded before broadcast.
ALTER TABLE proof_jobs
    ADD COLUMN proof_request_intent UUID,
    ADD COLUMN proof_bytes TEXT,
    ADD COLUMN prepared_transaction JSONB,
    ADD COLUMN error_class TEXT,
    ADD COLUMN next_attempt_at TIMESTAMPTZ,
    ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;

CREATE INDEX proof_jobs_ready_retry
    ON proof_jobs (next_attempt_at, created_at)
    WHERE status IN ('queued', 'approved');
