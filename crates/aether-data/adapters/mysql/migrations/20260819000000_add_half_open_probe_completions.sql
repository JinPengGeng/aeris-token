CREATE TABLE IF NOT EXISTS half_open_probe_completions (
    provider_key_id VARCHAR(191) NOT NULL,
    api_format VARCHAR(191) NOT NULL,
    completion_id VARCHAR(64) NOT NULL,
    owner VARCHAR(255) NOT NULL,
    fencing_token BIGINT NOT NULL,
    completed_at_unix_ms BIGINT NOT NULL,
    outcome VARCHAR(16) NOT NULL,
    PRIMARY KEY (provider_key_id, api_format),
    CONSTRAINT chk_half_open_probe_fencing_token CHECK (fencing_token > 0),
    CONSTRAINT chk_half_open_probe_completed_at CHECK (completed_at_unix_ms > 0),
    CONSTRAINT chk_half_open_probe_outcome CHECK (outcome IN ('succeeded', 'failed'))
);
