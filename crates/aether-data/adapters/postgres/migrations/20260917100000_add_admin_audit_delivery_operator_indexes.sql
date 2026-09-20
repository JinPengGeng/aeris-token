CREATE INDEX IF NOT EXISTS admin_audit_delivery_operator_page_idx
    ON admin_audit_delivery (created_at DESC, event_id DESC);

CREATE INDEX IF NOT EXISTS admin_audit_delivery_operator_state_page_idx
    ON admin_audit_delivery (state, created_at DESC, event_id DESC);
