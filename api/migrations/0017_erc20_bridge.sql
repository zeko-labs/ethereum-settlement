ALTER TABLE gateway_bridge_deposits
    ADD COLUMN asset_id TEXT;

ALTER TABLE gateway_inner_action_leaves
    ADD COLUMN token TEXT,
    ADD COLUMN asset_id TEXT,
    ADD CONSTRAINT gateway_inner_action_asset_pair
        CHECK ((token IS NULL) = (asset_id IS NULL));

CREATE INDEX gateway_inner_action_asset_recipient
    ON gateway_inner_action_leaves (token, recipient, global_action_index)
    WHERE token IS NOT NULL AND recipient IS NOT NULL AND NOT removed;
