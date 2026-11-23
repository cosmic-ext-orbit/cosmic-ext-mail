// SPDX-License-Identifier: GPL-3.0-only

use crate::fl;
use chrono::{Local, TimeZone};
use cosmic::{Element, widget};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    Inbox,
    Drafts,
    Sent,
    Archive,
    Junk,
    Trash,
}

impl Folder {
    pub const ALL: [Self; 6] = [
        Self::Inbox,
        Self::Drafts,
        Self::Sent,
        Self::Archive,
        Self::Junk,
        Self::Trash,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Inbox => fl!("inbox"),
            Self::Drafts => fl!("drafts"),
            Self::Sent => fl!("sent"),
            Self::Archive => fl!("archive-folder"),
            Self::Junk => fl!("junk-folder"),
            Self::Trash => fl!("trash-folder"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Inbox => "mail-folder-inbox-symbolic",
            Self::Drafts => "document-new-symbolic",
            Self::Sent => "mail-send-symbolic",
            Self::Archive => "mail-archive-symbolic",
            Self::Junk => "mail-mark-junk-symbolic",
            Self::Trash => "user-trash-symbolic",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailId {
    pub account: Uuid,
    pub folder: String,
    pub uid_validity: u32,
    pub uid: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mail {
    pub id: MailId,
    pub account_email: String,
    pub unread: bool,
    #[serde(default)]
    pub flagged: bool,
    pub sender: String,
    pub address: String,
    #[serde(default)]
    pub recipients: Option<Vec<Contact>>,
    pub subject: String,
    pub time: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Contact {
    pub name: String,
    pub address: String,
}

impl Contact {
    pub fn from_list(addresses: mail_parser::AddressList<'_>) -> Vec<Self> {
        addresses
            .mailboxes()
            .filter_map(|mailbox| {
                let name = mailbox.name().unwrap_or_default();
                let address = mailbox.address().unwrap_or_default();
                (!name.is_empty() || !address.is_empty()).then(|| Self {
                    name: name.to_owned(),
                    address: address.to_owned(),
                })
            })
            .collect()
    }
}

impl Mail {
    pub fn display_time(&self) -> String {
        let Some(date) = Local.timestamp_opt(self.timestamp, 0).single() else {
            return self.time.clone();
        };
        let format = if date.date_naive() == Local::now().date_naive() {
            "%H:%M"
        } else {
            "%Y-%m-%d"
        };
        date.format(format).to_string()
    }

    pub fn sender_picture<'a, Message: 'a>(
        &self,
        size: f32,
        photo: Option<&cosmic::iced::widget::image::Handle>,
    ) -> Element<'a, Message> {
        if let Some(photo) = photo {
            return cosmic::iced::widget::image(photo.clone())
                .width(size)
                .height(size)
                .border_radius(size / 2.0)
                .into();
        }
        let name = if self.sender.is_empty() {
            &self.address
        } else {
            &self.sender
        };
        let initials: String = name
            .split_whitespace()
            .filter_map(|word| word.chars().find(|character| character.is_alphanumeric()))
            .take(2)
            .flat_map(char::to_uppercase)
            .collect();
        let label = if initials.is_empty() {
            "?".into()
        } else {
            initials
        };
        widget::container(widget::text::caption(label))
            .center_x(size)
            .center_y(size)
            .class(cosmic::theme::Container::custom(move |theme| {
                let component = &theme.cosmic().accent;
                cosmic::iced::widget::container::Style {
                    background: Some(cosmic::iced::Color::from(component.base).into()),
                    text_color: Some(component.on.into()),
                    border: cosmic::iced::Border::default().rounded(size / 2.0),
                    ..Default::default()
                }
            }))
            .into()
    }

    pub fn matches(&self, query: &str) -> bool {
        let mut words = query.split_whitespace().peekable();
        if words.peek().is_none() {
            return true;
        }
        let text = format!("{} {} {}", self.sender, self.address, self.subject).to_lowercase();
        words.all(|word| text.contains(&word.to_lowercase()))
    }
}
