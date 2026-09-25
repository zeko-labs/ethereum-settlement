-- One owner spans preparation, proof generation, submission and finality.
-- The monotonically increasing token fences preparation attempts after expiry.
CREATE TABLE gateway_outer_writer (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    reservation_id UUID,
    owner_id TEXT,
    fencing_token BIGINT NOT NULL DEFAULT 0,
    expires_at TIMESTAMPTZ,
    checkpoint JSONB,
    job_id UUID REFERENCES proof_jobs(id),
    CHECK ((reservation_id IS NULL AND owner_id IS NULL AND expires_at IS NULL
            AND checkpoint IS NULL AND job_id IS NULL)
        OR (reservation_id IS NOT NULL AND owner_id IS NOT NULL
            AND (job_id IS NOT NULL OR expires_at IS NOT NULL)))
);

INSERT INTO gateway_outer_writer (id) VALUES (TRUE);
