CREATE TABLE gateway_explorer_settlements (
    id BIGSERIAL PRIMARY KEY,
    batch_sequence BIGINT NOT NULL,
    mina_transaction_hash TEXT NOT NULL,
    ledger_hash TEXT NOT NULL,
    outer_action_state TEXT NOT NULL,
    outer_action_state_length BIGINT NOT NULL,
    inner_action_state TEXT NOT NULL,
    inner_action_state_length BIGINT NOT NULL,
    slot_lower BIGINT NOT NULL,
    slot_upper BIGINT NOT NULL,
    inner_action_root TEXT,
    inner_action_start_index BIGINT,
    inner_action_count BIGINT,
    claimable_slot BIGINT,
    ethereum_block_number BIGINT NOT NULL,
    ethereum_block_hash TEXT NOT NULL,
    ethereum_tx_hash TEXT NOT NULL,
    ethereum_log_index BIGINT NOT NULL,
    removed BOOLEAN NOT NULL DEFAULT FALSE,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (ethereum_tx_hash, ethereum_log_index)
);

CREATE INDEX gateway_explorer_settlements_sequence
    ON gateway_explorer_settlements (batch_sequence DESC)
    WHERE NOT removed;

CREATE TABLE gateway_native_withdrawal_claims (
    settlement_sequence BIGINT NOT NULL,
    global_action_index BIGINT NOT NULL,
    recipient TEXT NOT NULL,
    zeko_amount NUMERIC(20, 0) NOT NULL,
    ethereum_amount NUMERIC(78, 0) NOT NULL,
    action_fields_hash TEXT NOT NULL,
    ethereum_block_number BIGINT NOT NULL,
    ethereum_block_hash TEXT NOT NULL,
    ethereum_tx_hash TEXT NOT NULL,
    ethereum_log_index BIGINT NOT NULL,
    removed BOOLEAN NOT NULL DEFAULT FALSE,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (settlement_sequence, global_action_index),
    UNIQUE (ethereum_tx_hash, ethereum_log_index)
);

CREATE INDEX gateway_native_withdrawal_claims_recipient
    ON gateway_native_withdrawal_claims (recipient, global_action_index DESC)
    WHERE NOT removed;

CREATE TABLE gateway_explorer_index_state (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    last_block BIGINT NOT NULL
);
