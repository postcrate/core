-- 0008_forwarding.sql — SMTP auto-forwarding rules.
--
-- Each rule says: "for every email captured in `mailbox_id`, forward
-- a copy to `target_addresses` (JSON array) via the SMTP relay
-- described by `relay_json`". A rule with `mailbox_id IS NULL` fires
-- for every mailbox.
--
-- We keep the relay config inline as JSON (rather than referencing a
-- shared relay table) because in practice each rule wants its own
-- credentials/host pair, and the rule + its target list is what the
-- user manages atomically.

CREATE TABLE forwarding_rules (
    id                TEXT PRIMARY KEY,
    mailbox_id        TEXT REFERENCES mailboxes(id) ON DELETE CASCADE,
    target_addresses  TEXT NOT NULL,
    relay_json        TEXT NOT NULL,
    enabled           INTEGER NOT NULL DEFAULT 1,
    created_at        INTEGER NOT NULL
);
CREATE INDEX idx_forwarding_mailbox ON forwarding_rules(mailbox_id);
CREATE INDEX idx_forwarding_enabled ON forwarding_rules(enabled) WHERE enabled = 1;
