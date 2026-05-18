package main

import (
	"log"
	"net/smtp"
	"os"
)

func main() {
	host := os.Getenv("POSTCRATE_SMTP_HOST")
	port := os.Getenv("POSTCRATE_SMTP_PORT")
	addr := host + ":" + port

	body := []byte(
		"From: go@example.com\r\n" +
			"To: rcpt-go@example.com\r\n" +
			"Subject: go interop test\r\n" +
			"Date: Mon, 1 Jan 2024 00:00:00 +0000\r\n" +
			"\r\n" +
			"Hello from Go's net/smtp.\r\n",
	)

	if err := smtp.SendMail(addr, nil, "go@example.com", []string{"rcpt-go@example.com"}, body); err != nil {
		log.Fatalf("smtp.SendMail: %v", err)
	}
}
