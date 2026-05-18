-- 0005_email_flags.sql — pin/star/note per FR-UX-40 and FR-UX-50.
--
-- All three live on the `emails` row instead of a side table: they're
-- per-email boolean/text flags, accessed alongside every other row
-- field, and join-free queries are simpler to maintain. `note` is
-- nullable; the two booleans default to 0.
--
-- `clear_mailbox` (see `db::emails`) honors `preserve_pinned` so a
-- user can keep important captures across a manual purge.

ALTER TABLE emails ADD COLUMN pinned   INTEGER NOT NULL DEFAULT 0;
ALTER TABLE emails ADD COLUMN starred  INTEGER NOT NULL DEFAULT 0;
ALTER TABLE emails ADD COLUMN note     TEXT;

CREATE INDEX idx_emails_pinned  ON emails(pinned)  WHERE pinned  = 1;
CREATE INDEX idx_emails_starred ON emails(starred) WHERE starred = 1;
