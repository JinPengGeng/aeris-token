CREATE TABLE IF NOT EXISTS half_open_probe_completions (
    provider_key_id TEXT NOT NULL,
    api_format TEXT NOT NULL,
    completion_id TEXT NOT NULL,
    owner TEXT NOT NULL,
    fencing_token INTEGER NOT NULL CHECK (fencing_token > 0),
    completed_at_unix_ms INTEGER NOT NULL CHECK (completed_at_unix_ms > 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'failed')),
    PRIMARY KEY (provider_key_id, api_format)
);
