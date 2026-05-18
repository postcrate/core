#!/usr/bin/env node
// Send one message through Node's built-in `net` module — no nodemailer
// dependency so we don't require `npm install` in CI. This still
// exercises Node's TCP path end-to-end.

const net = require("net");

const host = process.env.POSTCRATE_SMTP_HOST;
const port = Number(process.env.POSTCRATE_SMTP_PORT);

function send(sock, line) {
  return new Promise((resolve, reject) => {
    sock.write(line + "\r\n", err => (err ? reject(err) : resolve()));
  });
}

function expect(sock, prefix) {
  return new Promise((resolve, reject) => {
    let buf = "";
    const onData = chunk => {
      buf += chunk;
      while (true) {
        const i = buf.indexOf("\n");
        if (i < 0) return;
        const line = buf.slice(0, i + 1);
        buf = buf.slice(i + 1);
        if (!line.startsWith(prefix)) {
          sock.removeListener("data", onData);
          return reject(new Error(`expected ${prefix}, got ${line.trim()}`));
        }
        // Final line of a multi-line reply has a space at index 3.
        if (line.length < 4 || line[3] === " ") {
          sock.removeListener("data", onData);
          return resolve();
        }
      }
    };
    sock.on("data", onData);
    sock.once("error", reject);
  });
}

async function main() {
  const sock = net.connect({ host, port });
  await new Promise((res, rej) => {
    sock.once("connect", res);
    sock.once("error", rej);
  });
  await expect(sock, "220");
  await send(sock, "EHLO node");
  await expect(sock, "250");
  await send(sock, "MAIL FROM:<node@example.com>");
  await expect(sock, "250");
  await send(sock, "RCPT TO:<rcpt-node@example.com>");
  await expect(sock, "250");
  await send(sock, "DATA");
  await expect(sock, "354");
  await send(
    sock,
    [
      "From: node@example.com",
      "To: rcpt-node@example.com",
      "Subject: node interop test",
      "Date: Mon, 1 Jan 2024 00:00:00 +0000",
      "",
      "Hello from Node.",
      ".",
    ].join("\r\n"),
  );
  await expect(sock, "250");
  await send(sock, "QUIT");
  await expect(sock, "221");
  sock.end();
}

main().catch(err => {
  console.error(err);
  process.exit(1);
});
