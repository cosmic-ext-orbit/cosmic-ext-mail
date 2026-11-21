// SPDX-License-Identifier: GPL-3.0-only

use crate::fl;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    Inbox,
    Drafts,
    Sent,
    Archive,
    Trash,
}

impl Folder {
    pub const ALL: [Self; 5] = [
        Self::Inbox,
        Self::Drafts,
        Self::Sent,
        Self::Archive,
        Self::Trash,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Inbox => fl!("inbox"),
            Self::Drafts => fl!("drafts"),
            Self::Sent => fl!("sent"),
            Self::Archive => fl!("archive-folder"),
            Self::Trash => fl!("trash-folder"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Inbox => "mail-inbox-symbolic",
            Self::Drafts => "document-edit-symbolic",
            Self::Sent => "mail-outbox-symbolic",
            Self::Archive => "folder-symbolic",
            Self::Trash => "user-trash-symbolic",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MailId {
    pub account: Uuid,
    pub folder: String,
    pub uid_validity: u32,
    pub uid: u32,
}

#[derive(Clone, Debug)]
pub struct Mail {
    pub id: MailId,
    pub account_email: String,
    pub unread: bool,
    pub sender: String,
    pub address: String,
    pub subject: String,
    pub time: String,
    pub timestamp: i64,
}

impl Mail {
    pub fn matches(&self, query: &str) -> bool {
        let text = format!("{} {} {}", self.sender, self.address, self.subject).to_lowercase();
        query
            .split_whitespace()
            .all(|word| text.contains(&word.to_lowercase()))
    }
}
