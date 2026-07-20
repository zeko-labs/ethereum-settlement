CREATE TABLE gateway_account_history (
    job_id UUID NOT NULL REFERENCES proof_jobs(id) ON DELETE CASCADE,
    public_key TEXT NOT NULL,
    token_id TEXT NOT NULL,
    account_before JSONB NOT NULL,
    ethereum_block_number BIGINT NOT NULL,
    ethereum_block_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (job_id, public_key, token_id)
);

CREATE INDEX gateway_account_history_block
    ON gateway_account_history (ethereum_block_number);
