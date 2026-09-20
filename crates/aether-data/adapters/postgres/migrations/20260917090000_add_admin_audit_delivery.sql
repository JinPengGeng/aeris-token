CREATE TABLE IF NOT EXISTS admin_audit_delivery (
    event_id VARCHAR(36) PRIMARY KEY,
    payload JSONB NOT NULL,
    state VARCHAR(20) NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    lease_token UUID,
    lease_expires_at TIMESTAMPTZ,
    last_error_code VARCHAR(64),
    delivered_at TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT admin_audit_delivery_state_check
        CHECK (state IN ('pending', 'leased', 'delivered', 'dead_letter')),
    CONSTRAINT admin_audit_delivery_attempt_count_check CHECK (attempt_count >= 0),
    CONSTRAINT admin_audit_delivery_payload_object_check
        CHECK (JSONB_TYPEOF(payload) = 'object'),
    CONSTRAINT admin_audit_delivery_terminal_check CHECK (
        (state = 'delivered' AND delivered_at IS NOT NULL AND dead_lettered_at IS NULL)
        OR (state = 'dead_letter' AND dead_lettered_at IS NOT NULL AND delivered_at IS NULL)
        OR (state IN ('pending', 'leased') AND delivered_at IS NULL AND dead_lettered_at IS NULL)
    ),
    CONSTRAINT admin_audit_delivery_lease_check CHECK (
        (state = 'leased' AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (state <> 'leased' AND lease_token IS NULL AND lease_expires_at IS NULL)
    )
);

CREATE INDEX IF NOT EXISTS admin_audit_delivery_ready_idx
    ON admin_audit_delivery (next_attempt_at, created_at, event_id)
    WHERE state = 'pending';

CREATE INDEX IF NOT EXISTS admin_audit_delivery_expired_lease_idx
    ON admin_audit_delivery (lease_expires_at, event_id)
    WHERE state = 'leased';

CREATE INDEX IF NOT EXISTS admin_audit_delivery_unresolved_idx
    ON admin_audit_delivery (created_at, event_id)
    WHERE state IN ('pending', 'leased', 'dead_letter');
