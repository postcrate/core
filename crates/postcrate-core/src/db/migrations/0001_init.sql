-- 0001_init.sql — initial schema.
-- Tables are created in dependency order. All foreign keys cascade.

CREATE TABLE mailboxes (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    name         TEXT NOT NULL,
    port         INTEGER NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('primary', 'shared', 'ephemeral')),
    ttl_seconds  INTEGER,
    expires_at   INTEGER,
    failed       INTEGER NOT NULL DEFAULT 0,
    fail_reason  TEXT,
    created_at   INTEGER NOT NULL,
    UNIQUE (project_id, name),
    UNIQUE (port)
);
CREATE INDEX idx_mailboxes_kind        ON mailboxes(kind);
CREATE INDEX idx_mailboxes_project     ON mailboxes(project_id);
CREATE INDEX idx_mailboxes_expires_at  ON mailboxes(expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE emails (
    id              TEXT PRIMARY KEY,
    mailbox_id      TEXT NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
    received_at     INTEGER NOT NULL,
    smtp_from       TEXT NOT NULL,
    smtp_to_json    TEXT NOT NULL,
    header_from     TEXT,
    header_to       TEXT,
    header_cc       TEXT,
    header_subject  TEXT,
    message_id      TEXT,
    in_reply_to     TEXT,
    size_bytes      INTEGER NOT NULL,
    has_html        INTEGER NOT NULL DEFAULT 0,
    has_text        INTEGER NOT NULL DEFAULT 0,
    raw_path        TEXT NOT NULL,
    parsed_json     TEXT NOT NULL,
    read_flag       INTEGER NOT NULL DEFAULT 0,
    ext_smtputf8    INTEGER NOT NULL DEFAULT 0,
    ext_8bitmime    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_emails_mailbox_received ON emails(mailbox_id, received_at DESC);
CREATE INDEX idx_emails_message_id       ON emails(message_id);

CREATE TABLE attachments (
    id            TEXT PRIMARY KEY,
    email_id      TEXT NOT NULL REFERENCES emails(id) ON DELETE CASCADE,
    filename      TEXT,
    content_type  TEXT,
    content_id    TEXT,
    size_bytes    INTEGER NOT NULL,
    blob_path     TEXT NOT NULL
);
CREATE INDEX idx_attachments_email ON attachments(email_id);

CREATE TABLE bounce_rules (
    id               TEXT PRIMARY KEY,
    mailbox_id       TEXT NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
    address_pattern  TEXT NOT NULL,
    bounce_kind      TEXT NOT NULL CHECK (bounce_kind IN ('hard', 'soft')),
    smtp_code        INTEGER NOT NULL,
    smtp_message     TEXT NOT NULL,
    enabled          INTEGER NOT NULL DEFAULT 1,
    created_at       INTEGER NOT NULL
);
CREATE INDEX idx_bounce_rules_mailbox ON bounce_rules(mailbox_id);

CREATE TABLE chaos_configs (
    mailbox_id             TEXT PRIMARY KEY REFERENCES mailboxes(id) ON DELETE CASCADE,
    enabled                INTEGER NOT NULL DEFAULT 0,
    reject_4xx_prob        REAL    NOT NULL DEFAULT 0,
    reject_5xx_prob        REAL    NOT NULL DEFAULT 0,
    delay_ms_min           INTEGER NOT NULL DEFAULT 0,
    delay_ms_max           INTEGER NOT NULL DEFAULT 0,
    drop_during_data_prob  REAL    NOT NULL DEFAULT 0,
    malformed_resp_prob    REAL    NOT NULL DEFAULT 0,
    seed                   INTEGER
);

CREATE TABLE settings (
    section TEXT NOT NULL,
    key     TEXT NOT NULL,
    value   TEXT NOT NULL,
    PRIMARY KEY (section, key)
);
