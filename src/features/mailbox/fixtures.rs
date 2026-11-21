// SPDX-License-Identifier: GPL-3.0-only

use super::{Folder, Mail};
use cosmic_ext_mail::EmailBody;

pub fn sample_mail() -> Vec<Mail> {
    vec![
        Mail {
            folder: Folder::Inbox,
            unread: true,
            sender: "Northwind Books",
            address: "orders@example.invalid",
            subject: "Order #1042 confirmed",
            snippet: "Here’s your receipt for order #1042.",
            time: "10:42",
            attachment: None,
            body: EmailBody::Html(include_str!("../../../resources/sample-email.html")),
        },
        Mail {
            folder: Folder::Inbox,
            unread: true,
            sender: "Morgan Lee",
            address: "morgan@example.invalid",
            subject: "Friday plans",
            snippet: "Are we still on for coffee on Friday?",
            time: "09:18",
            attachment: None,
            body: EmailBody::PlainText(
                "Hi Eduardo,\n\nAre we still on for coffee on Friday? I can meet anytime after 3.\n\nMorgan",
            ),
        },
        Mail {
            folder: Folder::Inbox,
            unread: false,
            sender: "Avery Chen",
            address: "avery@example.invalid",
            subject: "Notes from the design review",
            snippet: "I’ve collected the feedback from everyone.",
            time: "Yesterday",
            attachment: None,
            body: EmailBody::PlainText(
                "Hi Eduardo,\n\nI’ve collected the feedback from everyone. The main follow-up is to test the layout in a half-tiled window.\n\nThanks,\nAvery",
            ),
        },
        Mail {
            folder: Folder::Inbox,
            unread: false,
            sender: "Casey Park",
            address: "casey@example.invalid",
            subject: "Schedule for next week",
            snippet: "The schedule is attached.",
            time: "Yesterday",
            attachment: Some("schedule.txt"),
            body: EmailBody::Mime(include_bytes!("../../../resources/sample-schedule.eml")),
        },
        Mail {
            folder: Folder::Sent,
            unread: false,
            sender: "Eduardo",
            address: "eduardo@example.invalid",
            subject: "Re: Design review",
            snippet: "Thanks for sharing the notes.",
            time: "Yesterday",
            attachment: None,
            body: EmailBody::PlainText(
                "Thanks for sharing the notes. I’ll take a look this afternoon.",
            ),
        },
        Mail {
            folder: Folder::Archive,
            unread: false,
            sender: "Jamie Rivera",
            address: "jamie@example.invalid",
            subject: "Welcome to the project",
            snippet: "Here’s what to expect this week.",
            time: "Monday",
            attachment: None,
            body: EmailBody::PlainText(
                "Welcome aboard! I’ll send over the agenda before our next meeting.",
            ),
        },
    ]
}
