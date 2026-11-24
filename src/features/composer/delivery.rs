// SPDX-License-Identifier: GPL-3.0-only

use super::{Draft, store::Store};
use crate::features::accounts::{self, Account};
use lettre::{
    Message, Transport,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
};

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Account(accounts::Error),
    Attachment,
    Message,
    Rejected,
    Uncertain,
}

pub(super) async fn send(store: Store, draft: Draft, account: Account) -> Result<(), Error> {
    let auth = accounts::authentication(&account)
        .await
        .map_err(Error::Account)?;
    tokio::task::spawn_blocking(move || {
        let message = build(&store, &draft, &account.email)?;
        let transport = accounts::smtp_transport(&account, auth).map_err(Error::Account)?;
        deliver(&transport, &message)
    })
    .await
    .map_err(|_| Error::Uncertain)?
}

fn deliver(transport: &lettre::SmtpTransport, message: &Message) -> Result<(), Error> {
    transport.send(message).map(|_| ()).map_err(|error| {
        if error.status().is_some() || error.is_client() || error.is_tls() {
            Error::Rejected
        } else {
            Error::Uncertain
        }
    })
}

fn build(store: &Store, draft: &Draft, from: &str) -> Result<Message, Error> {
    let from = from.parse::<Mailbox>().map_err(|_| Error::Message)?;
    let message_id = format!("<{}@{}>", draft.id, from.email.domain());
    let mut builder = Message::builder()
        .from(from)
        .subject(&draft.subject)
        .message_id(Some(message_id))
        .date_now();
    if let Some(reply_to) = &draft.reply_to {
        builder = builder
            .in_reply_to(reply_to.clone())
            .references(reply_to.clone());
    }
    for address in &draft.recipients[0] {
        builder = builder.to(address.parse().map_err(|_| Error::Message)?);
    }
    for address in &draft.recipients[1] {
        builder = builder.cc(address.parse().map_err(|_| Error::Message)?);
    }
    for address in &draft.recipients[2] {
        builder = builder.bcc(address.parse().map_err(|_| Error::Message)?);
    }

    if draft.attachments.is_empty() {
        return builder
            .singlepart(SinglePart::plain(draft.body.clone()))
            .map_err(|_| Error::Message);
    }
    let mut parts = MultiPart::mixed().singlepart(SinglePart::plain(draft.body.clone()));
    for file in &draft.attachments {
        let bytes = store
            .read_attachment(draft.id, file)
            .map_err(|_| Error::Attachment)?;
        parts = parts.singlepart(Attachment::new(file.name.clone()).body(
            bytes,
            ContentType::parse("application/octet-stream").expect("valid MIME type"),
        ));
    }
    builder.multipart(parts).map_err(|_| Error::Message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lettre::SmtpTransport;
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        time::Duration,
    };

    #[test]
    fn draft_round_trip_keeps_attachments_and_hides_bcc() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::for_test(directory.path());
        let mut draft = Draft::new(None);
        draft.subject = "Fixture".into();
        draft.body = "A synthetic message.".into();
        draft.recipients[0] = vec!["to@example.test".into()];
        draft.recipients[1] = vec!["cc@example.test".into()];
        draft.recipients[2] = vec!["hidden@example.test".into()];
        let original = directory.path().join("note.txt");
        fs::write(&original, "Attachment snapshot").unwrap();
        draft.attachments = store
            .import(draft.id, vec![original.clone()], 1024)
            .unwrap();
        fs::remove_file(original).unwrap();
        draft.delivery = super::super::Delivery::Pending;
        store.save(&draft).unwrap();

        let (restored, unreadable) = store.load().unwrap();
        assert!(!unreadable);
        let restored = &restored[0];
        assert_eq!(restored.body, draft.body);
        let reopened = super::super::Session::new(restored.clone());
        assert_eq!(reopened.draft.delivery, super::super::Delivery::Uncertain);
        assert_eq!(
            store
                .read_attachment(restored.id, &restored.attachments[0])
                .unwrap(),
            b"Attachment snapshot"
        );
        let message = build(&store, restored, "sender@example.test").unwrap();
        let raw = String::from_utf8(message.formatted()).unwrap();
        assert!(raw.contains("Content-Disposition: attachment"));
        assert!(raw.contains("note.txt"));
        assert!(raw.contains("Cc: cc@example.test"));
        assert!(!raw.contains("hidden@example.test"));
        assert!(
            message
                .envelope()
                .to()
                .iter()
                .any(|address| address.to_string() == "hidden@example.test")
        );
    }

    #[test]
    #[ignore = "requires loopback TCP"]
    fn smtp_distinguishes_acceptance_rejection_and_disconnect_after_data() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::for_test(directory.path());
        let mut draft = Draft::new(None);
        draft.recipients[0] = vec!["to@example.test".into()];
        draft.recipients[2] = vec!["hidden@example.test".into()];
        draft.body = "Synthetic mail body.".into();
        let message = build(&store, &draft, "sender@example.test").unwrap();

        for (final_reply, expected) in [
            (Some("250 accepted\r\n"), Ok(())),
            (Some("550 rejected\r\n"), Err(Error::Rejected)),
            (None, Err(Error::Uncertain)),
        ] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let worker = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut input = BufReader::new(socket.try_clone().unwrap());
                socket.write_all(b"220 fixture ESMTP\r\n").unwrap();
                let mut commands = String::new();
                let mut raw = String::new();
                loop {
                    let mut line = String::new();
                    if input.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    commands.push_str(&line);
                    let response = if line.starts_with("EHLO ") {
                        "250 fixture\r\n"
                    } else if line.starts_with("MAIL FROM:")
                        || line.starts_with("RCPT TO:")
                        || line.starts_with("RSET")
                    {
                        "250 OK\r\n"
                    } else if line == "DATA\r\n" {
                        socket.write_all(b"354 End with dot\r\n").unwrap();
                        loop {
                            let mut line = String::new();
                            assert_ne!(input.read_line(&mut line).unwrap(), 0);
                            if line == ".\r\n" {
                                break;
                            }
                            raw.push_str(&line);
                        }
                        let Some(response) = final_reply else { break };
                        response
                    } else if line == "QUIT\r\n" {
                        socket.write_all(b"221 Bye\r\n").unwrap();
                        break;
                    } else {
                        panic!("unexpected SMTP command {line:?}");
                    };
                    socket.write_all(response.as_bytes()).unwrap();
                }
                (commands, raw)
            });
            let transport = SmtpTransport::builder_dangerous("127.0.0.1")
                .port(port)
                .timeout(Some(Duration::from_secs(5)))
                .build();
            let result = deliver(&transport, &message);
            assert!(
                matches!(
                    (result, expected),
                    (Ok(()), Ok(()))
                        | (Err(Error::Rejected), Err(Error::Rejected))
                        | (Err(Error::Uncertain), Err(Error::Uncertain))
                ),
                "{result:?}"
            );
            drop(transport);
            let (commands, raw) = worker.join().unwrap();
            assert!(commands.contains("RCPT TO:<hidden@example.test>"));
            assert!(!raw.contains("hidden@example.test"));
            assert!(raw.contains("Synthetic mail body."));
        }
    }
}
