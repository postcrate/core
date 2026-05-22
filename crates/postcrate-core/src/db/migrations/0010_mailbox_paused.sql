-- User-intent flag distinct from the runtime "listener bound or not"
-- state and from the involuntary `failed` state. When set, the mailbox
-- still exists (same row, same port, same TTL), but its SMTP listener
-- is intentionally not bound. boot() skips paused mailboxes; the user
-- restores them by calling start_mailbox, which clears this flag.
ALTER TABLE mailboxes
    ADD COLUMN paused INTEGER NOT NULL DEFAULT 0;
