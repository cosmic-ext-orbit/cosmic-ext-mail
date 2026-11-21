// SPDX-License-Identifier: GPL-3.0-only

use super::{Folder, Mail, MailId};
use crate::{
    features::accounts::{self, Account},
    fl,
};
use cosmic::iced::futures::{StreamExt, stream};
use imap::types::{Flag, Name, NameAttribute};
use mail_parser::MessageParser;
use std::{cmp::Reverse, sync::Arc};

pub const PAGE_SIZE: u32 = 50;
const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Account(accounts::Error),
    Folder,
    Fetch,
    MessageUnavailable,
    MessageTooLarge,
}

impl From<accounts::Error> for Error {
    fn from(error: accounts::Error) -> Self {
        Self::Account(error)
    }
}

impl Error {
    pub fn text(self) -> String {
        match self {
            Self::Account(error) => error.text(),
            Self::Folder => fl!("mail-folder-failed"),
            Self::Fetch => fl!("mail-fetch-failed"),
            Self::MessageUnavailable => fl!("mail-unavailable"),
            Self::MessageTooLarge => fl!("mail-too-large"),
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct FolderContents {
    pub messages: Vec<Mail>,
    pub failures: Vec<(String, Error)>,
    pub limited: bool,
}

pub async fn load(accounts: Vec<Account>, folder: Folder) -> FolderContents {
    let mut results = stream::iter(accounts.into_iter().map(|account| async move {
        let email = account.email.clone();
        (email, fetch_headers(account, folder).await)
    }))
    .buffer_unordered(4);
    let mut contents = FolderContents::default();
    while let Some((email, result)) = results.next().await {
        match result {
            Ok((messages, limited)) => {
                contents.messages.extend(messages);
                contents.limited |= limited;
            }
            Err(error) => contents.failures.push((email, error)),
        }
    }
    contents.messages.sort_by_key(|mail| {
        (
            Reverse(mail.timestamp),
            mail.id.account,
            Reverse(mail.id.uid),
        )
    });
    contents.failures.sort_by(|a, b| a.0.cmp(&b.0));
    contents
}

async fn fetch_headers(account: Account, folder: Folder) -> Result<(Vec<Mail>, bool), Error> {
    let account_id = account.id;
    let account_email = account.email.clone();
    accounts::with_imap(account, move |session| {
        let path = if folder == Folder::Inbox {
            "INBOX".to_owned()
        } else {
            let names = session
                .list(Some(""), Some("*"))
                .map_err(|_| Error::Folder)?;
            let Some(name) = folder_name(names.iter(), folder) else {
                return Ok((Vec::new(), false));
            };
            name.to_owned()
        };
        let mailbox = session.examine(&path).map_err(|_| Error::Folder)?;
        if mailbox.exists == 0 {
            return Ok((Vec::new(), false));
        }
        let uid_validity = mailbox.uid_validity.ok_or(Error::Fetch)?;
        let first = mailbox.exists.saturating_sub(PAGE_SIZE - 1).max(1);
        let headers = session
            .fetch(
                format!("{first}:{}", mailbox.exists),
                "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])",
            )
            .map_err(|_| Error::Fetch)?;
        let parser = MessageParser::default();
        let messages = headers
            .iter()
            .filter_map(|item| {
                let uid = item.uid?;
                let header = parser.parse_headers(item.header()?);
                let from = header
                    .as_ref()
                    .and_then(|header| header.from())
                    .and_then(|from| from.first());
                let address = from
                    .as_ref()
                    .and_then(|from| from.address())
                    .unwrap_or_default();
                let date = item.internal_date();
                Some(Mail {
                    id: MailId {
                        account: account_id,
                        folder: path.clone(),
                        uid_validity,
                        uid,
                    },
                    account_email: account_email.clone(),
                    unread: !item.flags().contains(&Flag::Seen),
                    sender: from
                        .as_ref()
                        .and_then(|from| from.name())
                        .filter(|name| !name.is_empty())
                        .or_else(|| (!address.is_empty()).then_some(address))
                        .map_or_else(|| fl!("mail-unknown-sender"), str::to_owned),
                    address: address.to_owned(),
                    subject: header
                        .as_ref()
                        .and_then(|header| header.subject())
                        .filter(|subject| !subject.is_empty())
                        .map_or_else(|| fl!("mail-no-subject"), str::to_owned),
                    time: date
                        .map(|date| date.format("%Y-%m-%d").to_string())
                        .unwrap_or_default(),
                    timestamp: date.map_or(i64::MIN, |date| date.timestamp()),
                })
            })
            .collect();
        Ok((messages, first > 1))
    })
    .await
}

fn folder_name<'a>(
    names: impl Iterator<Item = &'a Name> + Clone,
    folder: Folder,
) -> Option<&'a str> {
    let (attribute, aliases): (&str, &[&str]) = match folder {
        Folder::Drafts => ("\\Drafts", &["Drafts", "Draft"]),
        Folder::Sent => ("\\Sent", &["Sent", "Sent Mail", "Sent Items"]),
        Folder::Archive => ("\\Archive", &["Archive", "Archives"]),
        Folder::Trash => ("\\Trash", &["Trash", "Deleted Items", "Deleted Messages"]),
        Folder::Inbox => return Some("INBOX"),
    };
    let selectable = names.filter(|name| !name.attributes().iter().any(|attr| {
        matches!(attr, NameAttribute::NoSelect)
            || matches!(attr, NameAttribute::Custom(value) if value.eq_ignore_ascii_case("\\Noselect"))
    }));
    selectable
        .clone()
        .find(|name| {
            name.attributes().iter().any(|attr| {
        matches!(attr, NameAttribute::Custom(value) if value.eq_ignore_ascii_case(attribute))
    })
        })
        .or_else(|| {
            selectable
                .filter_map(|name| {
                    let leaf = name
                        .delimiter()
                        .and_then(|delimiter| name.name().rsplit(delimiter).next())
                        .unwrap_or_else(|| name.name());
                    aliases
                        .iter()
                        .position(|alias| leaf.eq_ignore_ascii_case(alias))
                        .map(|priority| ((leaf != name.name(), priority), name))
                })
                .min_by_key(|(rank, _)| *rank)
                .map(|(_, name)| name)
        })
        .map(Name::name)
}

pub async fn body(account: Account, id: MailId) -> Result<Arc<[u8]>, Error> {
    accounts::with_imap(account, move |session| {
        let mailbox = session.examine(&id.folder).map_err(|_| Error::Folder)?;
        if mailbox.uid_validity != Some(id.uid_validity) {
            return Err(Error::MessageUnavailable);
        }
        let messages = session
            .uid_fetch(
                id.uid.to_string(),
                format!("(UID BODY.PEEK[]<0.{}>)", MAX_BODY_BYTES + 1),
            )
            .map_err(|_| Error::Fetch)?;
        let body = messages
            .iter()
            .find(|message| message.uid == Some(id.uid))
            .and_then(|message| message.body())
            .ok_or(Error::MessageUnavailable)?;
        if body.len() > MAX_BODY_BYTES {
            return Err(Error::MessageTooLarge);
        }
        Ok(Arc::from(body))
    })
    .await
}
