CREATE TABLE IF NOT EXISTS half_open_probe_completions (
    provider_key_id VARCHAR(191) NOT NULL,
    api_format VARCHAR(191) NOT NULL,
    completion_id VARCHAR(64) NOT NULL,
    owner VARCHAR(255) NOT NULL,
    fencing_token BIGINT NOT NULL CHECK (fencing_token > 0),
    completed_at_unix_ms BIGINT NOT NULL CHECK (completed_at_unix_ms > 0),
    outcome VARCHAR(16) NOT NULL CHECK (outcome IN ('succeeded', 'failed')),
    PRIMARY KEY (provider_key_id, api_format)
);
