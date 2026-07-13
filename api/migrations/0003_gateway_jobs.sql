ALTER TYPE proof_status ADD VALUE IF NOT EXISTS 'proof_requested';
ALTER TYPE proof_status ADD VALUE IF NOT EXISTS 'proof_failed';
ALTER TYPE proof_status ADD VALUE IF NOT EXISTS 'ethereum_reverted';
ALTER TYPE proof_status ADD VALUE IF NOT EXISTS 'reorged';
ALTER TYPE proof_status ADD VALUE IF NOT EXISTS 'rejected';

ALTER TABLE proof_jobs
    ADD COLUMN IF NOT EXISTS input_digest TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS cycle_count BIGINT,
    ADD COLUMN IF NOT EXISTS prover_gas BIGINT,
    ADD COLUMN IF NOT EXISTS base_fee_prove TEXT,
    ADD COLUMN IF NOT EXISTS max_price_per_pgu TEXT,
    ADD COLUMN IF NOT EXISTS actual_cost_prove TEXT,
    ADD COLUMN IF NOT EXISTS ethereum_gas_used BIGINT,
    ADD COLUMN IF NOT EXISTS confirmations INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS explorer_url TEXT;

CREATE INDEX IF NOT EXISTS proof_jobs_request_id
    ON proof_jobs (proof_request_id)
    WHERE proof_request_id IS NOT NULL;
