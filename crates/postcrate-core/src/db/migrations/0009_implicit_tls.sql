-- 0009_implicit_tls.sql — per-mailbox implicit-TLS flag (port-465 style).
--
-- When `implicit_tls = 1` and the engine was built with `--features
-- tls`, the listener for that mailbox wraps every accepted connection
-- in rustls *before* sending the SMTP banner. STARTTLS is *not*
-- offered inside an implicit-TLS session (per RFC 8314 §3.3 the
-- session is already encrypted).
--
-- Off by default to preserve plaintext+STARTTLS for the typical local
-- testing setup.

ALTER TABLE mailboxes ADD COLUMN implicit_tls INTEGER NOT NULL DEFAULT 0;
