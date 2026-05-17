//! Tiny raw-TCP RFC 5321 sender. Useful when `swaks`/`smtplib` aren't
//! handy — it depends only on Tokio.
//!
//! Build & run:
//!     cargo run --example send_test -- --host 127.0.0.1 --port 1025 \
//!         --from a@b --to c@d --subject hi --body "hello postcrate"

use std::io::Write;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[derive(Debug)]
struct Args {
    host: String,
    port: u16,
    from: String,
    to: Vec<String>,
    subject: String,
    body: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();

    let mut s = TcpStream::connect((args.host.as_str(), args.port)).await?;
    let (r, mut w) = s.split();
    let mut br = BufReader::new(r);

    expect(&mut br, "220").await?;
    send(&mut w, "EHLO send_test\r\n").await?;
    drain_multi(&mut br, "250").await?;

    send(&mut w, &format!("MAIL FROM:<{}>\r\n", args.from)).await?;
    expect(&mut br, "250").await?;
    for t in &args.to {
        send(&mut w, &format!("RCPT TO:<{t}>\r\n")).await?;
        expect(&mut br, "250").await?;
    }
    send(&mut w, "DATA\r\n").await?;
    expect(&mut br, "354").await?;
    let date = chrono::Utc::now().to_rfc2822();
    let body = format!(
        "From: {}\r\nTo: {}\r\nSubject: {}\r\nDate: {}\r\n\r\n{}\r\n.\r\n",
        args.from,
        args.to.join(", "),
        args.subject,
        date,
        dot_stuff(&args.body),
    );
    w.write_all(body.as_bytes()).await?;
    w.flush().await?;
    let line = read_line(&mut br).await?;
    println!("{}", line.trim_end());

    send(&mut w, "QUIT\r\n").await?;
    Ok(())
}

fn parse_args() -> Args {
    let mut host = "127.0.0.1".to_string();
    let mut port = 1025u16;
    let mut from = String::new();
    let mut to: Vec<String> = Vec::new();
    let mut subject = "test".to_string();
    let mut body = "hello".to_string();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => host = it.next().unwrap_or(host),
            "--port" => port = it.next().and_then(|s| s.parse().ok()).unwrap_or(port),
            "--from" => from = it.next().unwrap_or_default(),
            "--to" => to.push(it.next().unwrap_or_default()),
            "--subject" => subject = it.next().unwrap_or(subject),
            "--body" => body = it.next().unwrap_or(body),
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    if from.is_empty() || to.is_empty() {
        eprintln!("--from and at least one --to required");
        std::process::exit(2);
    }
    Args { host, port, from, to, subject, body }
}

async fn read_line<R>(br: &mut BufReader<R>) -> std::io::Result<String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut s = String::new();
    br.read_line(&mut s).await?;
    Ok(s)
}

async fn send<W: AsyncWriteExt + Unpin>(w: &mut W, s: &str) -> std::io::Result<()> {
    let _ = std::io::stdout().write_all(format!("C: {s}").as_bytes());
    w.write_all(s.as_bytes()).await
}

async fn expect<R>(br: &mut BufReader<R>, code: &str) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let line = read_line(br).await?;
    print!("S: {line}");
    if !line.starts_with(code) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("expected {code} got {line}"),
        ));
    }
    Ok(())
}

async fn drain_multi<R>(br: &mut BufReader<R>, code: &str) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    loop {
        let line = read_line(br).await?;
        print!("S: {line}");
        if !line.starts_with(code) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("expected {code} got {line}"),
            ));
        }
        // Multi-line continuation marker is `-`; final line uses space.
        if line.len() >= 4 && line.as_bytes()[3] == b' ' {
            return Ok(());
        }
    }
}

fn dot_stuff(body: &str) -> String {
    body.lines()
        .map(|l| if l.starts_with('.') { format!(".{l}") } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\r\n")
}
