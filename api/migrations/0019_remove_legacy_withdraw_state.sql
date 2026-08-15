-- Withdrawal claims are bound directly to settlement inner-action roots.
-- Bridge transitions no longer carry a separate withdrawal accumulator.
ALTER TABLE gateway_explorer_bridge_transitions
    DROP COLUMN IF EXISTS new_withdraw_state;
