ALTER TABLE proof_jobs
    ADD COLUMN preflight_input_digest TEXT,
    ADD COLUMN approval_input_digest TEXT,
    ADD COLUMN approval_max_pgu BIGINT,
    ADD COLUMN approval_max_price_per_pgu BIGINT,
    ADD COLUMN approval_base_fee_atto_prove TEXT,
    ADD COLUMN approval_network_max_price_per_pgu TEXT,
    ADD COLUMN approval_max_cost_atto_prove TEXT,
    ADD COLUMN approved_at TIMESTAMPTZ;

ALTER TABLE proof_jobs
    ADD CONSTRAINT proof_approval_complete CHECK (
        (approved_at IS NULL
         AND approval_input_digest IS NULL
         AND approval_max_pgu IS NULL
         AND approval_max_price_per_pgu IS NULL)
        OR
        (approved_at IS NOT NULL
         AND approval_input_digest IS NOT NULL
         AND approval_max_pgu > 0
         AND approval_max_price_per_pgu > 0)
    );

DROP INDEX one_active_settlement;
CREATE UNIQUE INDEX one_active_settlement
ON proof_jobs ((kind))
WHERE kind = 'settlement'
  AND status IN (
    'validating', 'awaiting_approval', 'approved', 'proof_requested',
    'proving', 'submitting', 'submitted'
  );

DROP INDEX one_active_bridge_batch;
CREATE UNIQUE INDEX one_active_bridge_batch
ON proof_jobs ((kind))
WHERE kind = 'bridge'
  AND status IN (
    'queued', 'validating', 'awaiting_approval', 'approved',
    'proof_requested', 'proving', 'submitting', 'submitted'
  );
