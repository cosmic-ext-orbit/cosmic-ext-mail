// SPDX-License-Identifier: GPL-3.0-only

use super::{
    Contact, Folder, Mail, MailId,
    cache::{self, Cache, Snapshot},
};
use crate::{
    features::accounts::{self, Account},
    fl,
};
use cosmic::iced::futures::{StreamExt, stream};
use imap::types::{Flag, Name, NameAttribute};
use mail_parser::MessageParser;
use std::{cmp::Reverse, collections::HashMap, net::TcpStream, path::PathBuf, sync::Arc};
use tokio::sync::{Semaphore, mpsc, oneshot};

type Session = imap::Session<native_tls::TlsStream<TcpStream>>;

const METADATA_BATCH_SIZE: usize = 500;
const HEADER_BATCH_SIZE: usize = 100;
const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;
const SMALL_BODY_BYTES: usize = 256 * 1024;
static SYNC_SLOTS: Semaphore = Semaphore::const_new(4);

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Account(accounts::Error),
    Folder,
    Fetch,
    MessageUnavailable,
    MessageTooLarge,
    DestinationUnavailable,
    ChangeFailed,
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
            Self::DestinationUnavailable => fl!("mail-destination-unavailable"),
            Self::ChangeFailed => fl!("mail-change-failed"),
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct FolderContents {
    pub messages: Vec<Mail>,
    pub failures: Vec<(String, Error)>,
}

pub(super) async fn cached(accounts: Vec<Account>, folder: Folder) -> Option<FolderContents> {
    tokio::task::spawn_blocking(move || {
        let mut contents = FolderContents::default();
        let mut found = false;
        for account in accounts {
            if let Some(snapshot) =
                Cache::for_account(&account).and_then(|cache| cache.snapshot(folder))
            {
                found = true;
                contents.add(&account.email, snapshot);
            }
        }
        contents.sort();
        found.then_some(contents)
    })
    .await
    .ok()
    .flatten()
}

impl FolderContents {
    fn add(&mut self, email: &str, snapshot: Snapshot) {
        self.messages
            .extend(snapshot.messages.into_iter().map(|mut mail| {
                if mail.account_email != email {
                    mail.account_email = email.to_owned();
                }
                mail
            }));
    }

    fn sort(&mut self) {
        self.messages.sort_by_key(|mail| {
            (
                Reverse(mail.timestamp),
                mail.id.account,
                Reverse(mail.id.uid),
            )
        });
        self.failures.sort_by(|a, b| a.0.cmp(&b.0));
    }
}

#[derive(Clone, Copy, Debug)]
pub enum DownloadEvent {
    Started(usize),
    Saved(usize),
    Finished,
}

pub(super) async fn load(
    accounts: Vec<Account>,
    folder: Folder,
    downloads: mpsc::UnboundedSender<DownloadEvent>,
) -> FolderContents {
    let mut results = stream::iter(accounts.into_iter().map(|account| {
        let downloads = downloads.clone();
        async move {
            let email = account.email.clone();
            match sync(account.clone(), folder, downloads).await {
                Ok(snapshot) => (email, Some(snapshot), None),
                Err(error) => {
                    let snapshot = tokio::task::spawn_blocking(move || {
                        Cache::for_account(&account).and_then(|cache| cache.snapshot(folder))
                    })
                    .await
                    .ok()
                    .flatten();
                    (email, snapshot, Some(error))
                }
            }
        }
    }))
    .buffer_unordered(4);
    let mut contents = FolderContents::default();
    while let Some((email, snapshot, error)) = results.next().await {
        if let Some(snapshot) = snapshot {
            contents.add(&email, snapshot);
        }
        if let Some(error) = error {
            contents.failures.push((email, error));
        }
    }
    contents.sort();
    contents
}

async fn sync(
    account: Account,
    folder: Folder,
    downloads: mpsc::UnboundedSender<DownloadEvent>,
) -> Result<Snapshot, Error> {
    let (send, receive) = oneshot::channel();
    let mut worker = tokio::spawn(async move {
        let permit = SYNC_SLOTS
            .acquire()
            .await
            .expect("mail sync semaphore stays open");
        accounts::with_imap_session(account.clone(), move |session| -> Result<(), Error> {
            let _permit = permit;
            let cache = Cache::for_account(&account);
            let lock = cache
                .as_ref()
                .and_then(|cache| cache.sync_lock(folder).ok());
            let cache = cache.filter(|_| lock.is_some());
            let previous = cache.as_ref().and_then(|cache| cache.snapshot(folder));
            let (snapshot, sizes) = sync_headers(session, &account, folder, previous)?;
            if let Some(cache) = &cache {
                let _ = cache.save_snapshot(folder, &snapshot);
            }
            drop(lock);
            let pending = cache.as_ref().map(|cache| {
                snapshot
                    .messages
                    .iter()
                    .rev()
                    .filter_map(|mail| {
                        let size = sizes
                            .get(&mail.id.uid)
                            .copied()
                            .flatten()
                            .map(|size| size as usize);
                        let path = cache.body_path(&mail.id);
                        (!path.is_file() && size.is_none_or(|size| size <= MAX_BODY_BYTES))
                            .then_some((mail.id.uid, size, path))
                    })
                    .collect::<Vec<_>>()
            });
            // Deliver the folder while this connection fills in missing bodies.
            let _ = send.send(snapshot);
            if let Some(pending) = pending.filter(|pending| !pending.is_empty()) {
                let _ = downloads.send(DownloadEvent::Started(pending.len()));
                prefetch_bodies(session, pending, &downloads);
                let _ = downloads.send(DownloadEvent::Finished);
            }
            Ok(())
        })
        .await
    });

    tokio::select! {
        biased;
        Ok(snapshot) = receive => Ok(snapshot),
        result = &mut worker => {
            result.map_err(|_| accounts::Error::ConnectionTask)??;
            Err(Error::Fetch)
        }
    }
}

fn sync_headers(
    session: &mut Session,
    account: &Account,
    folder: Folder,
    previous: Option<Snapshot>,
) -> Result<(Snapshot, HashMap<u32, Option<u32>>), Error> {
    let path = if folder == Folder::Inbox {
        "INBOX".to_owned()
    } else {
        let names = session
            .list(Some(""), Some("*"))
            .map_err(|_| Error::Folder)?;
        let Some(name) = folder_name(names.iter(), folder) else {
            return Ok(Default::default());
        };
        name.to_owned()
    };
    let mailbox = session.examine(&path).map_err(|_| Error::Folder)?;
    if mailbox.exists == 0 {
        return Ok(Default::default());
    }
    let uid_validity = mailbox.uid_validity.ok_or(Error::Fetch)?;
    let mut uids: Vec<_> = session
        .uid_search("ALL")
        .map_err(|_| Error::Fetch)?
        .into_iter()
        .collect();
    uids.sort_unstable();
    let mut current = HashMap::with_capacity(uids.len());
    for batch in uids.chunks(METADATA_BATCH_SIZE) {
        let flags = session
            .uid_fetch(uid_set(batch.iter().copied()), "(UID FLAGS RFC822.SIZE)")
            .map_err(|_| Error::Fetch)?;
        current.extend(flags.iter().filter_map(|item| {
            Some((
                item.uid?,
                (
                    !item.flags().contains(&Flag::Seen),
                    item.flags().contains(&Flag::Flagged),
                    item.size,
                ),
            ))
        }));
    }
    let mut headers = previous
        .into_iter()
        .flat_map(|snapshot| snapshot.messages)
        .filter(|mail| mail.id.folder == path && mail.id.uid_validity == uid_validity)
        .map(|mail| (mail.id.uid, mail))
        .collect::<HashMap<_, _>>();
    let missing: Vec<_> = uids
        .iter()
        .copied()
        .filter(|uid| {
            current.contains_key(uid)
                && headers
                    .get(uid)
                    .is_none_or(|mail| mail.recipients.is_none())
        })
        .collect();
    for batch in missing.chunks(HEADER_BATCH_SIZE) {
        let fetched = session
            .uid_fetch(
                uid_set(batch.iter().copied()),
                "(UID INTERNALDATE BODY.PEEK[HEADER])",
            )
            .map_err(|_| Error::Fetch)?;
        let parser = MessageParser::default();
        headers.extend(fetched.iter().filter_map(|item| {
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
            Some((
                uid,
                Mail {
                    id: MailId {
                        account: account.id,
                        folder: path.clone(),
                        uid_validity,
                        uid,
                    },
                    account_email: account.email.clone(),
                    unread: false,
                    flagged: false,
                    sender: from
                        .as_ref()
                        .and_then(|from| from.name())
                        .filter(|name| !name.is_empty())
                        .or_else(|| (!address.is_empty()).then_some(address))
                        .map_or_else(|| fl!("mail-unknown-sender"), str::to_owned),
                    address: address.to_owned(),
                    recipients: Some(
                        header
                            .as_ref()
                            .and_then(|header| header.to())
                            .map(Contact::from_list)
                            .unwrap_or_default(),
                    ),
                    subject: header
                        .as_ref()
                        .and_then(|header| header.subject())
                        .filter(|subject| !subject.is_empty())
                        .map_or_else(|| fl!("mail-no-subject"), str::to_owned),
                    time: date
                        .map(|date| date.format("%Y-%m-%d").to_string())
                        .unwrap_or_default(),
                    timestamp: date.map_or(i64::MIN, |date| date.timestamp()),
                },
            ))
        }));
    }
    let mut sizes = HashMap::with_capacity(current.len());
    let messages = uids
        .into_iter()
        .filter_map(|uid| {
            let (unread, flagged, size) = current.remove(&uid)?;
            let mut mail = headers.remove(&uid)?;
            mail.unread = unread;
            mail.flagged = flagged;
            sizes.insert(uid, size);
            Some(mail)
        })
        .collect();
    Ok((Snapshot { messages }, sizes))
}

fn uid_set(uids: impl Iterator<Item = u32>) -> String {
    uids.map(|uid| uid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn prefetch_bodies(
    session: &mut Session,
    pending: Vec<(u32, Option<usize>, PathBuf)>,
    downloads: &mpsc::UnboundedSender<DownloadEvent>,
) {
    let (small, large): (Vec<_>, Vec<_>) = pending
        .into_iter()
        .partition(|(_, size, _)| size.is_some_and(|size| size <= SMALL_BODY_BYTES));
    let batches = small
        .chunks(8)
        .map(|batch| (batch, SMALL_BODY_BYTES))
        .chain(large.chunks(1).map(|batch| (batch, MAX_BODY_BYTES)));
    for (batch, limit) in batches {
        let missing: Vec<_> = batch
            .iter()
            .filter(|(_, _, path)| !path.is_file())
            .collect();
        let mut saved = batch.len() - missing.len();
        if !missing.is_empty() {
            let Ok(fetched) = session.uid_fetch(
                uid_set(missing.iter().map(|(uid, _, _)| *uid)),
                format!("(UID BODY.PEEK[]<0.{}>)", limit + 1),
            ) else {
                break;
            };
            for &(uid, size, ref path) in missing {
                if let Some(body) = fetched
                    .iter()
                    .find(|message| message.uid == Some(uid))
                    .and_then(|message| message.body())
                    .filter(|body| {
                        body.len() <= limit && size.is_none_or(|size| size == body.len())
                    })
                    && cache::save_body(path, body).is_ok()
                {
                    saved += 1;
                }
            }
        }
        if saved > 0 {
            let _ = downloads.send(DownloadEvent::Saved(saved));
        }
    }
}

fn folder_name<'a>(
    names: impl Iterator<Item = &'a Name> + Clone,
    folder: Folder,
) -> Option<&'a str> {
    let (attribute, aliases): (&str, &[&str]) = match folder {
        Folder::Drafts => ("\\Drafts", &["Drafts", "Draft"]),
        Folder::Sent => ("\\Sent", &["Sent", "Sent Mail", "Sent Items"]),
        Folder::Archive => ("\\Archive", &["Archive", "Archives"]),
        Folder::Junk => ("\\Junk", &["Junk", "Spam", "Junk Email"]),
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

#[derive(Clone, Copy, Debug)]
pub enum Change {
    Move(Folder),
    Seen(bool),
    Flagged(bool),
}

pub async fn change(account: Account, id: MailId, change: Change) -> Result<(), Error> {
    accounts::with_imap_session(account, move |session| {
        let mailbox = session.select(&id.folder).map_err(|_| Error::Folder)?;
        if mailbox.uid_validity != Some(id.uid_validity) {
            return Err(Error::MessageUnavailable);
        }
        let uid = id.uid.to_string();
        match change {
            Change::Move(folder) => {
                let names = session
                    .list(Some(""), Some("*"))
                    .map_err(|_| Error::DestinationUnavailable)?;
                let destination = folder_name(names.iter(), folder)
                    .filter(|name| *name != id.folder)
                    .ok_or(Error::DestinationUnavailable)?;
                session
                    .uid_mv(uid, destination)
                    .map_err(|_| Error::ChangeFailed)?;
            }
            Change::Seen(seen) => {
                let command = if seen {
                    "+FLAGS.SILENT (\\Seen)"
                } else {
                    "-FLAGS.SILENT (\\Seen)"
                };
                session
                    .uid_store(uid, command)
                    .map_err(|_| Error::ChangeFailed)?;
            }
            Change::Flagged(flagged) => {
                let command = if flagged {
                    "+FLAGS.SILENT (\\Flagged)"
                } else {
                    "-FLAGS.SILENT (\\Flagged)"
                };
                session
                    .uid_store(uid, command)
                    .map_err(|_| Error::ChangeFailed)?;
            }
        }
        Ok(())
    })
    .await
}

pub async fn body(account: Account, id: MailId) -> Result<Arc<[u8]>, Error> {
    let path = Cache::for_account(&account).map(|cache| cache.body_path(&id));
    if let Some(path) = &path
        && let Ok(bytes) = tokio::fs::read(path).await
        && bytes.len() <= MAX_BODY_BYTES
    {
        return Ok(bytes.into());
    }
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
        if let Some(path) = path {
            let _ = cache::save_body(&path, body);
        }
        Ok(Arc::from(body))
    })
    .await
}
