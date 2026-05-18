-- 0004_fts_email_id.sql — rebuild `emails_fts` with an UNINDEXED
-- `email_id` column so search joins on a real column instead of relying
-- on a Rust-side rowid hash.
--
-- The previous schema (0002) used `fts_rowid(email_id)` (an FNV-1a hash)
-- as the FTS rowid so DELETEs could find the row. That worked for
-- DELETE, but made SEARCH awkward: MATCH returns rowids, and there was
-- no way to map those rowids back to email ids inside a single query.
-- With `email_id UNINDEXED` we just JOIN.
--
-- We backfill from `emails.parsed_json` for `body`; the JSON shape is
-- stable (see `mail::parse::Parsed` → `serde_json::Value`).

DROP TABLE IF EXISTS emails_fts;

CREATE VIRTUAL TABLE emails_fts USING fts5(
    subject,
    sender,
    recipients,
    body,
    email_id UNINDEXED,
    tokenize='unicode61 remove_diacritics 2'
);

INSERT INTO emails_fts(subject, sender, recipients, body, email_id)
SELECT
    COALESCE(header_subject, ''),
    smtp_from,
    COALESCE(smtp_to_json, ''),
    COALESCE(json_extract(parsed_json, '$.text_body'), ''),
    id
FROM emails;
