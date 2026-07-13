ALTER TYPE proof_status ADD VALUE IF NOT EXISTS 'submitted';

ALTER TABLE proof_jobs
    ADD COLUMN IF NOT EXISTS submitted_block_number BIGINT,
    ADD COLUMN IF NOT EXISTS submitted_block_hash TEXT;

CREATE INDEX IF NOT EXISTS proof_jobs_submitted_block
    ON proof_jobs (submitted_block_number)
    WHERE transaction_hash IS NOT NULL;
