-- 0003_audit.sql — append-only audit log of mutating actions.
-- Purged by retention; capped via settings.advanced.audit_retain_days.

CREATE TABLE audit_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    at            INTEGER NOT NULL,
    actor         TEXT NOT NULL,
    action        TEXT NOT NULL,
    target_kind   TEXT,
    target_id     TEXT,
    metadata_json TEXT
);
CREATE INDEX idx_audit_at ON audit_log(at DESC);
