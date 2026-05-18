# HTTP API

Versioned at `/api/v1`. Loopback by default; `0.0.0.0` only if `settings.network.exposeOnLan = true`. Body limit 50 MB. Request timeout 30 s. All JSON in **camelCase**.

When `settings.network.apiAuthToken` is set, every `/api/v1/...` endpoint requires `Authorization: Bearer <token>` (constant-time compared). `/healthz` and `/info` remain open for liveness probes. When `settings.network.apiTls` is on and the binary is built with `--features tls`, the API is served over HTTPS using the same cert/key pair as STARTTLS.

## Health

| Method | Path | Returns |
|---|---|---|
| GET | `/healthz` | `ok` (text) |
| GET | `/info` | `{ version, uptimeSec, runningMailboxes, bindHost, httpPort }` |

## Mailboxes

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/v1/mailboxes?projectId=…` | — | `[Mailbox]` |
| GET | `/api/v1/mailboxes/:id` | — | `Mailbox` |
| POST | `/api/v1/mailboxes` | `{ projectId, name, kind: "primary"\|"shared"\|"ephemeral", port?, ttlSeconds? }` | `Mailbox` |
| PATCH | `/api/v1/mailboxes/:id` | partial of above | `Mailbox` |
| DELETE | `/api/v1/mailboxes/:id` | — | `{ deleted: true }` |
| POST | `/api/v1/mailboxes/ephemeral` | `{ projectId, name?, ttlSeconds }` | `{ id, host, port, expiresAt }` |
| DELETE | `/api/v1/mailboxes/:id/messages` | — | `{ deleted: <count> }` |

## Messages

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/v1/messages?mailboxId=…&limit=&offset=` | — | `[EmailSummary]` |
| GET | `/api/v1/messages/:id` | — | `EmailDetail` |
| GET | `/api/v1/messages/:id/raw` | — | `message/rfc822` body |
| DELETE | `/api/v1/messages/:id` | — | `{ deleted: true }` |
| GET | `/api/v1/messages/:id/attachments/:aid` | — | attachment bytes (with `Content-Type` + `Content-Disposition`) |
| POST | `/api/v1/messages/search` | `{ q, mailboxId?, limit? }` | `[EmailSummary]` |
| POST | `/api/v1/messages/:id/read` | `{ read: bool }` | `{ read: bool }` |
| POST | `/api/v1/messages/:id/pin` | `{ pinned: bool }` | `{ pinned: bool }` |
| POST | `/api/v1/messages/:id/star` | `{ starred: bool }` | `{ starred: bool }` |
| POST | `/api/v1/messages/:id/note` | `{ note: string \| null }` | `{ note }` |
| POST | `/api/v1/messages/:id/tag` | `{ tag: string \| null }` | `{ tag }` |
| POST | `/api/v1/messages/:id/release` | `{ to: [string], relay: RelayConfig }` | `{ released: true }` |
| POST | `/api/v1/messages/:id/replay` | `{ targetMailboxId }` | `{ id: <new email id> }` |
| GET | `/api/v1/messages/:id/render?profile=...` | — | `{ html, fidelity, notes }` |
| GET | `/api/v1/messages/:id/lint` | — | `{ findings: [...] }` |
| GET | `/api/v1/messages/:id/a11y` | — | `{ findings: [...] }` |

Search uses SQLite FTS5 with the `unicode61` tokenizer (`remove_diacritics 2`). Whitespace-separated tokens combine with implicit AND. Each token is treated as a prefix term, so `"alic"` matches "alice". Hyphens are dropped from the query (they're an FTS5 operator). Searchable columns: `subject`, `sender`, `recipients`, `body`.

## Audit

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/v1/audit?limit=&offset=` | — | `[AuditEntry]` |
| DELETE | `/api/v1/audit?olderThanDays=` | — | `{ deleted: <count> }` |

`AuditEntry`:
```json
{
  "id": 42,
  "at": 1716071234567,
  "actor": "user",
  "action": "mailbox.create",
  "targetKind": "mailbox",
  "targetId": "...",
  "metadata": { "...": "..." }
}
```
`DELETE /audit` without `olderThanDays` clears the entire log. With `olderThanDays=30`, prunes entries older than 30 days.

## Chaos

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/v1/mailboxes/:id/chaos` | — | `ChaosConfig` |
| PUT | `/api/v1/mailboxes/:id/chaos` | `ChaosConfig` | `ChaosConfig` |

`ChaosConfig`:
```json
{
  "enabled": true,
  "reject4xxProb": 0.1,
  "reject5xxProb": 0.0,
  "delayMsMin": 0,
  "delayMsMax": 250,
  "dropDuringDataProb": 0.0,
  "malformedRespProb": 0.0,
  "seed": 42
}
```
With `seed` set, runs are deterministic.

## Bounces

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/v1/mailboxes/:id/bounces` | — | `[BounceRule]` |
| POST | `/api/v1/mailboxes/:id/bounces` | `BounceRule` (no `id`) | `BounceRule` |
| DELETE | `/api/v1/bounces/:ruleId` | — | `{ deleted: true }` |

`BounceRule`:
```json
{
  "id": "...",
  "mailboxId": "...",
  "addressPattern": "bounce@*",
  "bounceKind": "hard",
  "smtpCode": 550,
  "smtpMessage": "User unknown",
  "enabled": true
}
```
Pattern is a tiny glob: `*` matches any run of characters.

## Scenario checks

Static analysis passes that run against a captured message and report findings without modifying it.

| Method | Path | Returns |
|---|---|---|
| GET | `/api/v1/messages/:id/scenarios/spam` | `{ score, findings }` (local heuristics) |
| GET | `/api/v1/messages/:id/scenarios/links` | `{ links: [...], findings }` |
| GET | `/api/v1/messages/:id/scenarios/auth` | `{ spf, dkim, dmarc }` |
| GET | `/api/v1/messages/:id/scenarios/list-unsub` | `{ present, valid, uris, oneClick, findings }` (RFC 2369 / 8058) |

## Assertions

| Method | Path | Body | Returns |
|---|---|---|---|
| POST | `/api/v1/messages/wait` | `{ mailboxId, predicate, timeoutMs? }` | `EmailDetail` once a match arrives, else `408` |
| POST | `/api/v1/messages/:id/assert` | `EmailPredicate` | `{ matched: bool, ... }` |

## Server-sent events

| Method | Path | Returns |
|---|---|---|
| GET | `/api/v1/events?mailboxId=...` | `text/event-stream` of `CoreEvent` payloads |

## Mailbox import / export

| Method | Path | Body | Returns |
|---|---|---|---|
| POST | `/api/v1/mailboxes/:id/export` | `{ format: "postcrate" \| "eml-zip" }` | recording bytes (binary) |
| POST | `/api/v1/mailboxes/:id/import` | recording bytes | `{ imported: <count> }` |

## Webhooks

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/v1/webhooks?mailboxId=...` | — | `[Webhook]` |
| POST | `/api/v1/webhooks` | `CreateWebhook` | `Webhook` |
| DELETE | `/api/v1/webhooks/:id` | — | `{ deleted: true }` |

A webhook fires once per captured email. `mailboxId` is optional — a webhook with no `mailboxId` is global. `authHeader`, if set, is sent verbatim in the `Authorization` request header.

## Forwarding

| Method | Path | Body | Returns |
|---|---|---|---|
| GET | `/api/v1/forwarding?mailboxId=...` | — | `[ForwardingRule]` |
| POST | `/api/v1/forwarding` | `CreateForwardingRule` | `ForwardingRule` |
| DELETE | `/api/v1/forwarding/:id` | — | `{ deleted: true }` |

A forwarding rule re-sends each captured email to one or more downstream addresses via the configured `RelayConfig`. `relay.allowedRecipients`, if set, is a per-rule allow-list (glob patterns) checked against the `to` field at relay time.

## Errors

All errors return JSON `{ "error": "<code>", "message": "<human readable>" }` with status:

| Code | HTTP |
|---|---|
| `mailbox_not_found`, `email_not_found`, `attachment_not_found`, `bounce_rule_not_found` | 404 |
| `duplicate_mailbox`, `port_in_use` | 409 |
| `invalid_input`, `parse_error`, `port_out_of_range` | 400 |
| `not_implemented` | 501 |
| `port_range_exhausted` | 503 |
| anything else | 500 |
