-- 0006_email_tag.sql — auto-detected tag column on the emails row.
--
-- One tag per email; the classifier (`core::tagging`) picks the
-- strongest match. NULL means "not classified yet". The UI can
-- always re-derive on demand from headers/subject/body, but we
-- snapshot at ingest time so the list query stays cheap and so
-- search by tag is a covered index lookup.

ALTER TABLE emails ADD COLUMN tag TEXT;
CREATE INDEX idx_emails_tag ON emails(tag) WHERE tag IS NOT NULL;
