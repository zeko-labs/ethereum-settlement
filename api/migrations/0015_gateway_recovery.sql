-- Bridge transitions are protocol state, not merely proof-job bookkeeping.
-- Retain their canonical Ethereum event metadata so a fresh gateway database
-- can replay the exact accepted calldata into its Mina-compatible view.
CREATE TABLE gateway_explorer_bridge_transitions (
    id BIGSERIAL PRIMARY KEY,
    old_action_state TEXT NOT NULL,
    new_action_state TEXT NOT NULL,
    new_deposit_state TEXT NOT NULL,
    new_withdraw_state TEXT NOT NULL,
    new_deposit_nonce BIGINT NOT NULL,
    ethereum_block_number BIGINT NOT NULL,
    ethereum_block_hash TEXT NOT NULL,
    ethereum_tx_hash TEXT NOT NULL,
    ethereum_log_index BIGINT NOT NULL,
    removed BOOLEAN NOT NULL DEFAULT FALSE,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (ethereum_tx_hash, ethereum_log_index)
);

CREATE INDEX gateway_explorer_bridge_transitions_block
    ON gateway_explorer_bridge_transitions (
        ethereum_block_number, ethereum_log_index
    )
    WHERE NOT removed;

-- Existing databases may already have advanced the shared event cursor before
-- this event type existed. Replaying all configured blocks is idempotent.
DELETE FROM gateway_explorer_index_state;
