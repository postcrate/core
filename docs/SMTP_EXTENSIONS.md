# SMTP extensions

## EHLO advertisements

| Keyword | Behavior |
|---|---|
| `PIPELINING` | RFC 2920. Clients may batch commands; we read line-by-line so it works transparently. |
| `SIZE <max>` | RFC 1870. `<max>` is `settings.advanced.maxMessageBytes` (default 50 MB). Enforced both at envelope (`MAIL FROM SIZE=`) and mid-DATA via a byte counter. Violations return `552 Message size exceeds fixed maximum`. |
| `8BITMIME` | RFC 6152. We're byte-transparent regardless. |
| `SMTPUTF8` | RFC 6531. Byte-transparent; flag recorded on envelope so the UI can show "this message used internationalized addresses". |
| `ENHANCEDSTATUSCODES` | RFC 2034. Advertised; we don't yet add the `x.y.z` enhanced codes to replies. Reserved for a later pass. |
| `HELP` | Returns the list of supported commands on `HELP`. |
| `STARTTLS` | **Not advertised today.** The `tls` feature flag is reserved; see `docs/ARCHITECTURE.md`. |

## Commands

| Verb | Behavior |
|---|---|
| `HELO` / `EHLO` | Reset the session; advertise capabilities (EHLO multi-line, HELO single-line). |
| `MAIL FROM:<…>` | Accepts the SIZE / BODY / SMTPUTF8 ESMTP params per RFC 1869. Empty path `<>` is the null sender (RFC 5321 §3.3). |
| `RCPT TO:<…>` | Empty path rejected with `553`. Bounce rules consulted before accepting. |
| `DATA` | `354` start prompt; reads until `.\r\n`; un-dot-stuffs (RFC 5321 §4.5.2); 1000-octet line limit. |
| `RSET` | Reset envelope state, keep the session. |
| `NOOP` | Always `250`. |
| `QUIT` | `221` + close. |
| `VRFY` | `252 Cannot VRFY user; try RCPT` (RFC 5321 §3.5.3). |
| `HELP` | Multi-line `214` listing supported verbs. |
| `STARTTLS` | `502 Command not implemented` until the TLS phase. |
| Anything else | `502 Command not implemented`. Out-of-sequence commands return `503 Bad sequence`. |

## Limits

- Line length: 1000 octets including CRLF (`smtp_max_line_bytes`).
- Message size: `max_message_bytes` (default 50 MB).
- Spill threshold: messages larger than `data_spill_bytes` (default 256 KiB) stream to a temp file in `blobs/raw/incoming/`.
- Concurrent connections: one Tokio task per accepted socket; no global cap (the OS file-descriptor limit is the practical ceiling).
