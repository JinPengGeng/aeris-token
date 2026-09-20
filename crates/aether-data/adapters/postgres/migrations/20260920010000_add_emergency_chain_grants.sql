CREATE TABLE IF NOT EXISTS public.emergency_chain_grants (
    grant_id VARCHAR(256) PRIMARY KEY,
    principal VARCHAR(256) NOT NULL,
    operations JSONB NOT NULL CHECK (JSONB_TYPEOF(operations) = 'array' AND JSONB_ARRAY_LENGTH(operations) > 0),
    request_id VARCHAR(256) NOT NULL,
    request_fingerprint CHAR(64) NOT NULL CHECK (request_fingerprint ~ '^[0-9a-f]{64}$'),
    session_nonce VARCHAR(256) NOT NULL,
    chain_hash CHAR(64) NOT NULL CHECK (chain_hash ~ '^[0-9a-f]{64}$'),
    issued_at_unix_secs BIGINT NOT NULL CHECK (issued_at_unix_secs >= 0),
    expires_at_unix_secs BIGINT NOT NULL CHECK (
        expires_at_unix_secs > issued_at_unix_secs
        AND expires_at_unix_secs <= issued_at_unix_secs + 86400
    ),
    revoked_at_unix_secs BIGINT CHECK (revoked_at_unix_secs >= issued_at_unix_secs),
    consumed_at_unix_secs BIGINT CHECK (consumed_at_unix_secs >= issued_at_unix_secs),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE IF NOT EXISTS public.emergency_chain_grant_targets (
    grant_id VARCHAR(256) NOT NULL REFERENCES public.emergency_chain_grants(grant_id) ON DELETE RESTRICT,
    chain_position INTEGER NOT NULL CHECK (chain_position >= 0),
    provider_id VARCHAR(256) NOT NULL,
    endpoint_id VARCHAR(256) NOT NULL,
    key_id VARCHAR(256) NOT NULL,
    PRIMARY KEY (grant_id, chain_position),
    UNIQUE (grant_id, provider_id, endpoint_id, key_id)
);

CREATE INDEX IF NOT EXISTS emergency_chain_grants_request_id_idx
    ON public.emergency_chain_grants (request_id, expires_at_unix_secs);
