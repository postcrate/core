#!/usr/bin/env python3
"""Send one message through Python's stdlib smtplib."""

import os
import smtplib
from email.message import EmailMessage

host = os.environ["POSTCRATE_SMTP_HOST"]
port = int(os.environ["POSTCRATE_SMTP_PORT"])

msg = EmailMessage()
msg["From"] = "smtplib@example.com"
msg["To"] = "rcpt-python@example.com"
msg["Subject"] = "python smtplib test"
msg.set_content("Hello from Python's smtplib.\n")

with smtplib.SMTP(host, port, timeout=5) as s:
    s.send_message(msg)
