CREATE TABLE gateway_bridge_deposits (
    nonce BIGINT PRIMARY KEY CHECK (nonce > 0),
    deposit_leaf TEXT NOT NULL,
    old_deposit_state TEXT NOT NULL,
    new_deposit_state TEXT NOT NULL,
    token TEXT NOT NULL,
    sender TEXT NOT NULL,
    zeko_recipient TEXT NOT NULL,
    ethereum_amount NUMERIC(78, 0) NOT NULL CHECK (ethereum_amount > 0),
    zeko_amount NUMERIC(78, 0) NOT NULL CHECK (zeko_amount > 0),
    timeout BIGINT NOT NULL,
    ethereum_block_number BIGINT NOT NULL,
    ethereum_block_hash TEXT NOT NULL,
    ethereum_tx_hash TEXT NOT NULL,
    ethereum_log_index BIGINT NOT NULL,
    removed BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (ethereum_tx_hash, ethereum_log_index)
);

CREATE INDEX gateway_bridge_deposits_canonical_nonce
    ON gateway_bridge_deposits (nonce) WHERE NOT removed;

CREATE TABLE gateway_inner_action_leaves (
    settlement_sequence BIGINT NOT NULL,
    action_offset INTEGER NOT NULL,
    global_action_index BIGINT NOT NULL,
    action_fields JSONB NOT NULL,
    action_fields_hash TEXT NOT NULL,
    leaf TEXT NOT NULL,
    recipient TEXT,
    zeko_amount NUMERIC(20, 0),
    inner_action_root TEXT NOT NULL,
    commit_slot_upper BIGINT NOT NULL,
    ethereum_block_number BIGINT NOT NULL,
    ethereum_block_hash TEXT NOT NULL,
    ethereum_tx_hash TEXT NOT NULL,
    removed BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (settlement_sequence, action_offset),
    CHECK ((recipient IS NULL) = (zeko_amount IS NULL))
);

CREATE INDEX gateway_inner_action_recipient
    ON gateway_inner_action_leaves (recipient, global_action_index)
    WHERE recipient IS NOT NULL AND NOT removed;

-- The bridge receipt contains Mina action fields and Poseidon checkpoints, but
-- Mina GraphQL still needs the outer account address under which to expose
-- them. It is seeded from VIRTUAL_MINA_OUTER_PUBLIC_KEY and refreshed from
-- every confirmed sequencer settlement submission.
ALTER TABLE gateway_config ADD COLUMN outer_public_key TEXT;

-- A canonical bridge job covers every finalized nonce after the contract's
-- proven cursor. Allowing a second job before the first confirms would buy a
-- duplicate proof for the same custody range.
CREATE UNIQUE INDEX one_active_bridge_batch
    ON proof_jobs ((kind))
    WHERE kind = 'bridge'
      AND status IN (
        'queued', 'validating', 'proof_requested', 'proving',
        'submitting', 'submitted'
      );
