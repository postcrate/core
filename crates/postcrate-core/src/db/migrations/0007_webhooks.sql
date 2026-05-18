-- 0007_webhooks.sql — outbound webhooks fired on new captured email.
--
-- Two scopes: a row with `mailbox_id` IS NULL is a global webhook
-- (fires for every captured email regardless of mailbox); a row with
-- a specific `mailbox_id` fires only for that mailbox. Both forms
-- coexist; a single email can match both.
--
-- We keep this small: URL + optional auth header + on/off flag. The
-- ingest worker POSTs a JSON body matching `EmailSummary` to each
-- enabled URL after insert. Failures are logged, audit-recorded, and
-- *do not* fail the ingest itself — webhooks are best-effort.

CREATE TABLE webhooks (
    id            TEXT PRIMARY KEY,
    mailbox_id    TEXT REFERENCES mailboxes(id) ON DELETE CASCADE,
    url           TEXT NOT NULL,
    auth_header   TEXT,
    enabled       INTEGER NOT NULL DEFAULT 1,
    created_at    INTEGER NOT NULL
);
CREATE INDEX idx_webhooks_mailbox ON webhooks(mailbox_id);
CREATE INDEX idx_webhooks_enabled ON webhooks(enabled) WHERE enabled = 1;
