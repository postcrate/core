# HTTP API

Versioned at `/api/v1`. Loopback by default; `0.0.0.0` only if `settings.network.exposeOnLan = true`. Body limit 50 MB. Request timeout 30 s. All JSON in **camelCase**.

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
