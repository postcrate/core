-- 0002_fts.sql — full-text search virtual table.
-- Synced manually in `db::emails` because the searchable bodies live in
-- the parsed JSON, not directly in the `emails` row. We deliberately
-- do NOT use `content=''` (contentless) here: plain `DELETE` on a
-- contentless table requires the caller to pass the old column values,
-- which is awkward to track. The storage cost (a second copy of the
-- subject/sender/recipients/body strings) is acceptable at our volumes.

CREATE VIRTUAL TABLE emails_fts USING fts5(
    subject,
    sender,
    recipients,
    body,
    tokenize='unicode61 remove_diacritics 2'
);
