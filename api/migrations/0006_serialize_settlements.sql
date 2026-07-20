-- A proof commits to the next L1 batch number. Keep at most one settlement in
-- flight so a later job cannot pay for a proof whose sequence becomes stale.
CREATE UNIQUE INDEX one_active_settlement
ON proof_jobs ((kind))
WHERE kind = 'settlement'
  AND status IN (
    'queued', 'validating', 'proof_requested', 'proving',
    'submitting', 'submitted'
  );
