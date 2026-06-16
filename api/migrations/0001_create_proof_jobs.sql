CREATE TYPE proof_kind AS ENUM ('settlement', 'bridge', 'withdraw');
CREATE TYPE proof_status AS ENUM (
    'queued',
    'validating',
    'proving',
    'submitting',
    'executed',
    'confirmed',
    'failed'
);

CREATE TABLE proof_jobs (
    id UUID PRIMARY KEY,
    kind proof_kind NOT NULL,
    status proof_status NOT NULL DEFAULT 'queued',
    idempotency_key TEXT,
    input JSONB NOT NULL,
    public_values TEXT,
    proof_request_id TEXT,
    transaction_hash TEXT,
    error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX proof_jobs_idempotency_key
    ON proof_jobs (idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX proof_jobs_queue
    ON proof_jobs (status, created_at);
