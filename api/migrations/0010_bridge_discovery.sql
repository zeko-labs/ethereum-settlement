ALTER TABLE gateway_bridge_deposits
    ADD COLUMN bridge_job_id UUID REFERENCES proof_jobs(id),
    ADD COLUMN outer_action_sequence BIGINT,
    ADD COLUMN outer_action_state_after TEXT,
    ADD COLUMN synchronized_settlement_job_id UUID REFERENCES proof_jobs(id),
    ADD COLUMN synchronized_settlement_sequence BIGINT;

ALTER TABLE gateway_bridge_deposits
    ADD CONSTRAINT gateway_bridge_deposit_bridge_binding
        CHECK (
            (bridge_job_id IS NULL AND outer_action_sequence IS NULL
             AND outer_action_state_after IS NULL)
            OR
            (bridge_job_id IS NOT NULL AND outer_action_sequence IS NOT NULL
             AND outer_action_state_after IS NOT NULL)
        ),
    ADD CONSTRAINT gateway_bridge_deposit_settlement_binding
        CHECK (
            (synchronized_settlement_job_id IS NULL
             AND synchronized_settlement_sequence IS NULL)
            OR
            (synchronized_settlement_job_id IS NOT NULL
             AND synchronized_settlement_sequence IS NOT NULL)
        );

CREATE INDEX gateway_bridge_deposits_outer_action_sequence
    ON gateway_bridge_deposits (outer_action_sequence)
    WHERE NOT removed AND outer_action_sequence IS NOT NULL;

CREATE INDEX gateway_bridge_deposits_bridge_job
    ON gateway_bridge_deposits (bridge_job_id)
    WHERE bridge_job_id IS NOT NULL;
