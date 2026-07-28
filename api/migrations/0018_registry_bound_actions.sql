ALTER TABLE gateway_bridge_deposits
    ADD COLUMN action_encoding_version INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN registry_index BIGINT,
    ADD COLUMN record_commitment TEXT,
    ADD CONSTRAINT gateway_bridge_deposit_action_identity
        CHECK (
            action_encoding_version IN (0, 1, 2)
            AND (
                (action_encoding_version = 2
                    AND registry_index IS NOT NULL
                    AND record_commitment IS NOT NULL)
                OR
                (action_encoding_version <> 2
                    AND registry_index IS NULL
                    AND record_commitment IS NULL)
            )
        );

ALTER TABLE gateway_inner_action_leaves
    ADD COLUMN action_encoding_version INTEGER,
    ADD COLUMN registry_index BIGINT,
    ADD COLUMN record_commitment TEXT,
    ADD CONSTRAINT gateway_inner_action_registry_identity
        CHECK (
            action_encoding_version IS NULL
            OR (
                action_encoding_version IN (1, 2)
                AND (
                    (action_encoding_version = 2
                        AND registry_index IS NOT NULL
                        AND record_commitment IS NOT NULL)
                    OR
                    (action_encoding_version = 1
                        AND registry_index IS NULL
                        AND record_commitment IS NULL)
                )
            )
        );

-- Existing ERC-20 rows predate the registry-bound V2 wire and are retained as
-- explicit V1 compatibility data. Native/raw rows keep their zero/null tags.
UPDATE gateway_bridge_deposits
SET action_encoding_version = 1
WHERE token <> '0x0000000000000000000000000000000000000000';

UPDATE gateway_inner_action_leaves
SET action_encoding_version = 1
WHERE token IS NOT NULL;
