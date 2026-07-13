CREATE TABLE gateway_config (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    genesis_timestamp TEXT NOT NULL,
    fork_slot INTEGER NOT NULL,
    account_creation_fee TEXT NOT NULL,
    block_height BIGINT NOT NULL DEFAULT 0,
    state_hash TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE gateway_accounts (
    public_key TEXT NOT NULL,
    token_id TEXT NOT NULL,
    account_json JSONB NOT NULL,
    ethereum_block_number BIGINT,
    ethereum_block_hash TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (public_key, token_id)
);

CREATE TABLE gateway_actions (
    id BIGSERIAL PRIMARY KEY,
    address TEXT NOT NULL,
    sequence BIGINT NOT NULL,
    state_before TEXT NOT NULL,
    state_after TEXT NOT NULL,
    action_data JSONB NOT NULL,
    ethereum_block_number BIGINT NOT NULL,
    ethereum_block_hash TEXT NOT NULL,
    ethereum_tx_hash TEXT NOT NULL,
    ethereum_log_index BIGINT NOT NULL,
    removed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (ethereum_tx_hash, ethereum_log_index, sequence)
);

CREATE INDEX gateway_actions_address_sequence
    ON gateway_actions (address, sequence)
    WHERE NOT removed;

CREATE TABLE gateway_pending_commands (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id UUID REFERENCES proof_jobs(id) ON DELETE CASCADE,
    public_key TEXT NOT NULL,
    nonce BIGINT NOT NULL,
    command_kind TEXT NOT NULL CHECK (command_kind IN ('zkapp', 'signed')),
    command_base64 TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (job_id, command_kind)
);

CREATE INDEX gateway_pending_commands_public_key
    ON gateway_pending_commands (public_key, nonce);

CREATE TABLE gateway_blocks (
    block_number BIGINT PRIMARY KEY,
    block_hash TEXT NOT NULL UNIQUE,
    parent_hash TEXT NOT NULL,
    finalized BOOLEAN NOT NULL DEFAULT FALSE,
    canonical BOOLEAN NOT NULL DEFAULT TRUE,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
