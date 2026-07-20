-- A proof job is assigned as soon as the canonical deposit batch is queued so
-- public clients can observe proving and failure states. The action mapping is
-- still populated only after the Ethereum bridge transition is confirmed.
ALTER TABLE gateway_bridge_deposits
    DROP CONSTRAINT gateway_bridge_deposit_bridge_binding;

ALTER TABLE gateway_bridge_deposits
    ADD CONSTRAINT gateway_bridge_deposit_bridge_binding
        CHECK (
            (outer_action_sequence IS NULL AND outer_action_state_after IS NULL)
            OR
            (bridge_job_id IS NOT NULL AND outer_action_sequence IS NOT NULL
             AND outer_action_state_after IS NOT NULL)
        );

CREATE INDEX gateway_bridge_deposits_recipient_nonce
    ON gateway_bridge_deposits (zeko_recipient, nonce)
    WHERE NOT removed;
