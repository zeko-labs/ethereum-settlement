-- Ethereum-domain context is assigned when a worker claims a queued OCaml
-- commit. Multiple commits may therefore wait in order, while only one may
-- enter proving/submission and reserve the live next-batch context.
DROP INDEX one_active_settlement;

CREATE UNIQUE INDEX one_active_settlement
ON proof_jobs ((kind))
WHERE kind = 'settlement'
  AND status IN (
    'validating', 'proof_requested', 'proving',
    'submitting', 'submitted'
  );
