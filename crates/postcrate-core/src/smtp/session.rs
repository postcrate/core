//! Per-connection SMTP state machine. One task per accepted connection.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::error::Result;
use crate::mail::address::ParsedPath;
use crate::smtp::bounce::BounceEvaluator;
use crate::smtp::chaos::ChaosInjector;
use crate::smtp::codec::LineReader;
use crate::smtp::command::SmtpCommand;
use crate::smtp::data_reader::{read_data, DataOutcome, DataReadCfg};
use crate::smtp::extensions::EhloAdvert;
use crate::smtp::response::{ReplyWriter, SmtpReply};

/// What the session asks the caller to do next.
///
/// `Closed` is the common case (peer hung up or sent QUIT). `UpgradeTls`
/// hands the raw stream back to the listener so it can wrap it in a
/// rustls `TlsAcceptor::accept` call and start a fresh session on top.
#[derive(Debug)]
pub enum SessionOutcome<Io> {
    Closed,
    UpgradeTls(Io),
}

/// Sent to the ingest worker when DATA completes successfully.
#[derive(Debug)]
pub struct CapturedEnvelope {
    pub mailbox_id: String,
    pub received_at: i64,
    pub mail_from: String,
    pub rcpt_to: Vec<String>,
    pub raw: crate::smtp::data_reader::CapturedSource,
    pub ext_smtputf8: bool,
    pub ext_8bitmime: bool,
}

/// Per-session context — cheap to clone (mostly Arcs).
#[derive(Clone)]
pub struct SessionCtx {
    pub mailbox_id: String,
    pub ehlo_advert: EhloAdvert,
    pub max_line: usize,
    pub max_bytes: u64,
    pub spill_at: usize,
    pub incoming_dir: PathBuf,
    pub chaos: ChaosInjector,
    pub bounce: Arc<parking_lot::RwLock<BounceEvaluator>>,
    pub ingest_tx: mpsc::Sender<CapturedEnvelope>,
    /// True when this session is running on top of a TLS stream. When
    /// set, STARTTLS replies `503` (already active) and the EHLO advert
    /// drops STARTTLS so a polite client doesn't try a second upgrade.
    pub tls_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Greeted,
    MailFrom,
    RcptTo,
}

/// Run an accepted SMTP session to completion.
pub async fn run_session<Io>(io: Io, ctx: SessionCtx) -> Result<SessionOutcome<Io>>
where
    Io: AsyncRead + AsyncWrite + Unpin,
{
    let (reader_half, writer_half) = tokio::io::split(io);
    let mut reader = LineReader::new(reader_half, ctx.max_line);
    let mut writer = ReplyWriter::new(writer_half);

    // Chaos hook: pre-banner delay / malformed greeting.
    apply_delay(&ctx.chaos).await;
    if let Some(bytes) = ctx.chaos.maybe_malformed_bytes() {
        writer.send_raw(&bytes).await?;
        return Ok(SessionOutcome::Closed);
    }
    writer.send(&SmtpReply::greet(&ctx.ehlo_advert.hostname)).await?;

    let mut state = State::Greeted;
    let mut mail_from: Option<ParsedPath> = None;
    let mut rcpts: Vec<ParsedPath> = Vec::new();
    let mut ext_smtputf8 = false;
    let mut ext_8bitmime = false;

    loop {
        let Some(line) = reader.next_line().await? else {
            break;
        };

        // Per-command delay.
        apply_delay(&ctx.chaos).await;

        // Parse first so we can match on QUIT/RSET regardless of state.
        let parsed = match SmtpCommand::parse(&line) {
            Ok(c) => c,
            Err(_) => {
                writer.send(&SmtpReply::syntax_error()).await?;
                continue;
            }
        };

        // Universal handlers.
        match &parsed {
            SmtpCommand::Quit => {
                writer.send(&SmtpReply::bye()).await?;
                break;
            }
            SmtpCommand::Rset => {
                state = State::Greeted;
                mail_from = None;
                rcpts.clear();
                ext_smtputf8 = false;
                ext_8bitmime = false;
                writer.send(&SmtpReply::ok()).await?;
                continue;
            }
            SmtpCommand::Noop => {
                writer.send(&SmtpReply::ok()).await?;
                continue;
            }
            SmtpCommand::Vrfy(_) => {
                writer.send(&SmtpReply::vrfy_cannot()).await?;
                continue;
            }
            SmtpCommand::Help(_) => {
                writer.send(&SmtpReply::help_lines()).await?;
                continue;
            }
            SmtpCommand::StartTls => {
                if ctx.tls_active {
                    writer.send(&SmtpReply::tls_already_active()).await?;
                    continue;
                }
                if !ctx.ehlo_advert.starttls_enabled {
                    writer.send(&SmtpReply::tls_not_available()).await?;
                    continue;
                }
                writer.send(&SmtpReply::start_tls_ready()).await?;
                // Recover the raw stream and hand it back to the listener.
                // After this point the client expects the next bytes on the
                // wire to be a ClientHello.
                let reader_half = reader.into_inner();
                let writer_half = writer.into_inner();
                let stream = reader_half.unsplit(writer_half);
                return Ok(SessionOutcome::UpgradeTls(stream));
            }
            _ => {}
        }

        // Chaos: unconditional rejection roll (any command).
        if let Some(reject) = ctx.chaos.maybe_reject() {
            writer.send(&reject).await?;
            continue;
        }

        match (state, parsed) {
            (_, SmtpCommand::Helo(_)) => {
                writer.send(&SmtpReply::ok_msg(format!(
                    "{} hello",
                    ctx.ehlo_advert.hostname
                ))).await?;
                state = State::Greeted;
            }
            (_, SmtpCommand::Ehlo(client)) => {
                // Suppress the STARTTLS advert once we're already inside
                // a TLS session — RFC 3207 §4 wants us to omit it.
                let advert = if ctx.tls_active {
                    let mut a = ctx.ehlo_advert.clone();
                    a.starttls_enabled = false;
                    a
                } else {
                    ctx.ehlo_advert.clone()
                };
                writer.send(&advert.reply(&client)).await?;
                state = State::Greeted;
            }
            (State::Greeted, SmtpCommand::MailFrom(path)) => {
                // SIZE pre-check from the envelope.
                if let Some(declared) = path.extensions.size {
                    if declared > ctx.max_bytes {
                        writer.send(&SmtpReply::size_exceeded()).await?;
                        continue;
                    }
                }
                ext_smtputf8 = path.extensions.smtputf8;
                ext_8bitmime = path.extensions.body_8bitmime;
                mail_from = Some(path);
                state = State::MailFrom;
                writer.send(&SmtpReply::ok_msg("Sender OK")).await?;
            }
            (State::MailFrom | State::RcptTo, SmtpCommand::RcptTo(path)) => {
                // Empty recipient is invalid (RFC 5321 §3.3).
                if path.mailbox.is_empty() {
                    writer.send(&SmtpReply::custom(553, "Empty recipient not allowed")).await?;
                    continue;
                }
                // Bounce rules. Compute the reply in a scope so the guard
                // is dropped before we cross an await.
                let bounce_reply = {
                    let g = ctx.bounce.read();
                    g.match_recipient(&path.mailbox)
                };
                if let Some(reply) = bounce_reply {
                    writer.send(&reply).await?;
                    continue;
                }
                rcpts.push(path);
                state = State::RcptTo;
                writer.send(&SmtpReply::ok_msg("Recipient OK")).await?;
            }
            (State::RcptTo, SmtpCommand::Data) => {
                writer.send(&SmtpReply::start_mail_input()).await?;
                let cfg = DataReadCfg {
                    max_line: ctx.max_line,
                    max_bytes: ctx.max_bytes,
                    spill_at: ctx.spill_at,
                    spill_dir: ctx.incoming_dir.clone(),
                };
                match read_data(&mut reader, &cfg).await? {
                    DataOutcome::Complete(raw) => {
                        // Chaos: maybe drop the connection right before our 250.
                        if ctx.chaos.should_drop_during_data() {
                            return Ok(SessionOutcome::Closed);
                        }

                        let envelope = CapturedEnvelope {
                            mailbox_id: ctx.mailbox_id.clone(),
                            received_at: Utc::now().timestamp_millis(),
                            mail_from: mail_from
                                .as_ref()
                                .map(|p| p.mailbox.clone())
                                .unwrap_or_default(),
                            rcpt_to: rcpts.iter().map(|p| p.mailbox.clone()).collect(),
                            raw,
                            ext_smtputf8,
                            ext_8bitmime,
                        };

                        if ctx.ingest_tx.send(envelope).await.is_err() {
                            writer.send(&SmtpReply::transaction_failed()).await?;
                        } else {
                            writer.send(&SmtpReply::ok_msg("Message accepted")).await?;
                        }

                        // Reset for the next mail in this session.
                        state = State::Greeted;
                        mail_from = None;
                        rcpts.clear();
                        ext_smtputf8 = false;
                        ext_8bitmime = false;
                    }
                    DataOutcome::SizeExceeded => {
                        writer.send(&SmtpReply::size_exceeded()).await?;
                        // Drop the session — we don't know how to recover.
                        return Ok(SessionOutcome::Closed);
                    }
                    DataOutcome::Eof => return Ok(SessionOutcome::Closed),
                }
            }
            (_, SmtpCommand::MailFrom(_)) => {
                writer.send(&SmtpReply::bad_sequence()).await?;
            }
            (_, SmtpCommand::RcptTo(_)) => {
                writer.send(&SmtpReply::bad_sequence()).await?;
            }
            (_, SmtpCommand::Data) => {
                writer.send(&SmtpReply::bad_sequence()).await?;
            }
            (_, SmtpCommand::Unknown(_)) => {
                writer.send(&SmtpReply::command_not_implemented()).await?;
            }
            (_, SmtpCommand::Quit | SmtpCommand::Rset | SmtpCommand::Noop)
            | (_, SmtpCommand::Vrfy(_))
            | (_, SmtpCommand::Help(_))
            | (_, SmtpCommand::StartTls) => {
                // Handled above; unreachable in well-formed code.
            }
        }
    }

    Ok(SessionOutcome::Closed)
}

async fn apply_delay(chaos: &ChaosInjector) {
    if let Some(d) = chaos.delay() {
        tokio::time::sleep(d).await;
    }
}
