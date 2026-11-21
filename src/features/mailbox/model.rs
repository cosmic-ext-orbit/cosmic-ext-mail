// SPDX-License-Identifier: GPL-3.0-only

use crate::fl;
use cosmic_ext_mail::EmailBody;

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

pub struct Mail {
    pub folder: Folder,
    pub unread: bool,
    pub sender: &'static str,
    pub address: &'static str,
    pub subject: &'static str,
    pub snippet: &'static str,
    pub time: &'static str,
    pub body: EmailBody<'static>,
    pub attachment: Option<&'static str>,
}

impl Mail {
    pub fn matches(&self, query: &str) -> bool {
        let text = format!(
            "{} {} {} {}",
            self.sender, self.address, self.subject, self.snippet
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| text.contains(&word.to_lowercase()))
    }
}
