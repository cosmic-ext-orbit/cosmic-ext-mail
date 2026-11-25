// SPDX-License-Identifier: GPL-3.0-only

use super::{
    Change, Contact, Folder, Mail, MailId,
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
const FIRST_HEADER_BATCH_SIZE: usize = 50;
const HEADER_BATCH_SIZE: usize = 250;
const HEADER_UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
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
    PermanentDeleteUnsupported,
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
            Self::PermanentDeleteUnsupported => fl!("mail-permanent-delete-unsupported"),
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

    pub(super) fn sort(&mut self) {
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

#[derive(Clone, Debug)]
pub enum DownloadEvent {
    HeadersStarted(usize),
    HeadersFetched(usize),
    HeadersReady(Vec<Mail>),
    HeadersFinished,
    Started(usize),
    Saved(usize),
    Finished,
}

struct HeaderProgress<'a>(&'a mpsc::UnboundedSender<DownloadEvent>);

impl Drop for HeaderProgress<'_> {
    fn drop(&mut self) {
        let _ = self.0.send(DownloadEvent::HeadersFinished);
    }
}

pub(super) async fn load(
    accounts: Vec<Account>,
    folder: Folder,
    downloads: mpsc::UnboundedSender<DownloadEvent>,
    prefetch: bool,
    stream_headers: bool,
) -> FolderContents {
    let mut results = stream::iter(accounts.into_iter().map(|account| {
        let downloads = downloads.clone();
        async move {
            let email = account.email.clone();
            match sync(account.clone(), folder, downloads, prefetch, stream_headers).await {
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

pub(crate) async fn poll_inbox(accounts: Vec<Account>) -> bool {
    let (downloads, _progress) = mpsc::unbounded_channel();
    load(accounts, Folder::Inbox, downloads, false, false)
        .await
        .failures
        .is_empty()
}

async fn sync(
    account: Account,
    folder: Folder,
    downloads: mpsc::UnboundedSender<DownloadEvent>,
    prefetch: bool,
    stream_headers: bool,
) -> Result<Snapshot, Error> {
    if account.microsoft_graph {
        let _permit = SYNC_SLOTS
            .acquire()
            .await
            .expect("mail sync semaphore stays open");
        return super::graph::sync(account, folder, downloads, stream_headers).await;
    }
    let (send, receive) = oneshot::channel();
    let mut worker = tokio::spawn(async move {
        let permit = SYNC_SLOTS
            .acquire()
            .await
            .expect("mail sync semaphore stays open");
        if downloads.is_closed() {
            return Ok(());
        }
        accounts::with_imap_session(account.clone(), move |session| -> Result<(), Error> {
            let _permit = permit;
            let cache = Cache::for_account(&account);
            let lock = cache
                .as_ref()
                .and_then(|cache| cache.sync_lock(folder).ok());
            let cache = cache.filter(|_| lock.is_some());
            let previous = cache.as_ref().and_then(|cache| cache.snapshot(folder));
            let (snapshot, sizes) = sync_headers(
                session,
                &account,
                folder,
                previous,
                cache.as_ref(),
                &downloads,
                stream_headers,
            )?;
            if downloads.is_closed() {
                return Ok(());
            }
            if let Some(cache) = &cache {
                let _ = cache.save_snapshot(folder, &snapshot);
                if folder == Folder::Inbox
                    && let Ok(count) = cache.new_inbox_messages(&snapshot)
                    && count > 0
                {
                    tokio::spawn(super::notifications::new_mail(account.id, count));
                }
            }
            drop(lock);
            let pending = cache
                .as_ref()
                .filter(|_| prefetch && !downloads.is_closed())
                .map(|cache| {
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
    session: &mut imap::Session<impl std::io::Read + std::io::Write>,
    account: &Account,
    folder: Folder,
    previous: Option<Snapshot>,
    cache: Option<&Cache>,
    downloads: &mpsc::UnboundedSender<DownloadEvent>,
    stream_headers: bool,
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
    let uid_validity = mailbox.uid_validity.ok_or(Error::Fetch)?;
    if mailbox.exists == 0 {
        return Ok((
            Snapshot {
                uid_validity: Some(uid_validity),
                ..Snapshot::default()
            },
            HashMap::new(),
        ));
    }
    let mut uids: Vec<_> = session
        .uid_search("ALL")
        .map_err(|_| Error::Fetch)?
        .into_iter()
        .collect();
    uids.sort_unstable();
    let mut headers = previous
        .into_iter()
        .flat_map(|snapshot| snapshot.messages)
        .filter(|mail| {
            mail.id.folder == path
                && mail.id.uid_validity == uid_validity
                && uids.binary_search(&mail.id.uid).is_ok()
        })
        .map(|mail| (mail.id.uid, mail))
        .collect::<HashMap<_, _>>();
    let (mut cached, missing): (Vec<_>, Vec<_>) = uids.iter().rev().copied().partition(|uid| {
        headers
            .get(uid)
            .is_some_and(|mail| mail.recipients.is_some() && mail.thread.is_some())
    });
    let mut sizes = HashMap::with_capacity(uids.len());
    let header_progress = (!missing.is_empty()).then(|| {
        let _ = downloads.send(DownloadEvent::HeadersStarted(missing.len()));
        HeaderProgress(downloads)
    });

    let parser = MessageParser::default();
    let mut pending = Vec::new();
    let mut last_update = None;
    let mut fetched_since_checkpoint = false;
    let mut processed = 0;
    let mut next_checkpoint = FIRST_HEADER_BATCH_SIZE;
    let first = missing.len().min(FIRST_HEADER_BATCH_SIZE);
    let batches = [&missing[..first]]
        .into_iter()
        .filter(|batch| !batch.is_empty())
        .chain(missing[first..].chunks(HEADER_BATCH_SIZE));
    for batch in batches {
        let fetched = if downloads.is_closed() {
            Err(Error::Fetch)
        } else {
            session
                .uid_fetch(
                    uid_set(batch.iter().copied()),
                    "(UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])",
                )
                .map_err(|_| Error::Fetch)
        };
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(error) => {
                if fetched_since_checkpoint {
                    save_header_checkpoint(cache, folder, uid_validity, &headers);
                }
                return Err(error);
            }
        };
        for item in fetched.iter() {
            let (Some(uid), Some(bytes)) = (item.uid, item.header()) else {
                continue;
            };
            if batch.binary_search_by(|probe| uid.cmp(probe)).is_err() {
                continue;
            }
            let header = parser.parse_headers(bytes);
            let from = header
                .as_ref()
                .and_then(|header| header.from())
                .and_then(|from| from.first());
            let address = from
                .as_ref()
                .and_then(|from| from.address())
                .unwrap_or_default();
            let date = item.internal_date();
            let mail = Mail {
                id: MailId {
                    account: account.id,
                    folder: path.clone(),
                    uid_validity,
                    uid,
                    graph_id: None,
                },
                account_email: account.email.clone(),
                unread: !item.flags().contains(&Flag::Seen),
                flagged: item.flags().contains(&Flag::Flagged),
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
                thread: Some(header.as_ref().map_or_else(
                    super::threads::ThreadHeaders::default,
                    super::threads::ThreadHeaders::from_headers,
                )),
            };
            if stream_headers {
                pending.push(mail.clone());
            }
            sizes.insert(uid, item.size);
            headers.insert(uid, mail);
            fetched_since_checkpoint = true;
        }
        processed += batch.len();
        if !pending.is_empty()
            && last_update
                .is_none_or(|last: std::time::Instant| last.elapsed() >= HEADER_UPDATE_INTERVAL)
        {
            let _ = downloads.send(DownloadEvent::HeadersReady(std::mem::take(&mut pending)));
            last_update = Some(std::time::Instant::now());
        }
        let _ = downloads.send(DownloadEvent::HeadersFetched(batch.len()));

        if fetched_since_checkpoint && processed >= next_checkpoint && processed < missing.len() {
            save_header_checkpoint(cache, folder, uid_validity, &headers);
            fetched_since_checkpoint = false;
            next_checkpoint = processed.saturating_mul(2);
        }
    }
    if !pending.is_empty() {
        let _ = downloads.send(DownloadEvent::HeadersReady(pending));
    }
    drop(header_progress);
    if downloads.is_closed() {
        if fetched_since_checkpoint {
            save_header_checkpoint(cache, folder, uid_validity, &headers);
        }
        return Err(Error::Fetch);
    }
    cached.reverse();
    for batch in cached.chunks(METADATA_BATCH_SIZE) {
        let flags = if downloads.is_closed() {
            Err(Error::Fetch)
        } else {
            session
                .uid_fetch(uid_set(batch.iter().copied()), "(UID FLAGS RFC822.SIZE)")
                .map_err(|_| Error::Fetch)
        };
        let flags = match flags {
            Ok(flags) => flags,
            Err(error) => {
                if fetched_since_checkpoint {
                    save_header_checkpoint(cache, folder, uid_validity, &headers);
                }
                return Err(error);
            }
        };
        for item in flags.iter() {
            let Some(uid) = item.uid else { continue };
            if batch.binary_search(&uid).is_err() {
                continue;
            }
            if let Some(mail) = headers.get_mut(&uid) {
                mail.unread = !item.flags().contains(&Flag::Seen);
                mail.flagged = item.flags().contains(&Flag::Flagged);
                sizes.insert(uid, item.size);
            }
        }
    }
    let messages = uids
        .into_iter()
        .filter(|uid| sizes.contains_key(uid))
        .filter_map(|uid| headers.remove(&uid))
        .collect();
    Ok((
        Snapshot {
            messages,
            uid_validity: Some(uid_validity),
        },
        sizes,
    ))
}

fn save_header_checkpoint(
    cache: Option<&Cache>,
    folder: Folder,
    uid_validity: u32,
    headers: &HashMap<u32, Mail>,
) {
    if let Some(cache) = cache {
        let mut messages: Vec<_> = headers.values().cloned().collect();
        messages.sort_unstable_by_key(|mail| mail.id.uid);
        let _ = cache.save_snapshot(
            folder,
            &Snapshot {
                messages,
                uid_validity: Some(uid_validity),
            },
        );
    }
}

fn uid_set(uids: impl Iterator<Item = u32>) -> String {
    use std::fmt::Write;

    let mut uids = uids.peekable();
    let mut set = String::new();
    while let Some(first) = uids.next() {
        let mut last = first;
        while let Some(&next) = uids.peek() {
            if next.abs_diff(last) != 1 || (last != first && (next > last) != (last > first)) {
                break;
            }
            last = uids.next().expect("peeked UID");
        }
        if !set.is_empty() {
            set.push(',');
        }
        if last == first {
            let _ = write!(set, "{first}");
        } else {
            let _ = write!(set, "{}:{}", first.min(last), first.max(last));
        }
    }
    set
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
        if downloads.is_closed() {
            break;
        }
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

pub(super) async fn change(
    account: Account,
    folder: Folder,
    id: MailId,
    change: Change,
) -> Result<(), Error> {
    if account.microsoft_graph {
        return super::graph::change(account, folder, id, change).await;
    }
    let cache = Cache::for_account(&account);
    accounts::with_imap_session(account, move |session| {
        let cache = cache.as_ref().and_then(|cache| {
            let lock = cache.sync_lock(folder).ok()?;
            Some((cache, lock))
        });
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
            Change::DeletePermanently => {
                if folder != Folder::Trash {
                    return Err(Error::MessageUnavailable);
                }
                let names = session
                    .list(Some(""), Some("*"))
                    .map_err(|_| Error::Folder)?;
                if folder_name(names.iter(), Folder::Trash) != Some(id.folder.as_str()) {
                    return Err(Error::MessageUnavailable);
                }
                let capabilities = session.capabilities().map_err(|_| Error::ChangeFailed)?;
                if !["UIDPLUS", "IMAP4rev2", "IMAP4REV2"]
                    .into_iter()
                    .any(|name| capabilities.has_str(name))
                {
                    return Err(Error::PermanentDeleteUnsupported);
                }
                let flags = session
                    .uid_fetch(&uid, "(UID FLAGS)")
                    .map_err(|_| Error::ChangeFailed)?;
                if let Some(message) = flags.iter().find(|message| message.uid == Some(id.uid)) {
                    let already_deleted = message.flags().contains(&imap::types::Flag::Deleted);
                    if !already_deleted {
                        session
                            .uid_store(&uid, "+FLAGS.SILENT (\\Deleted)")
                            .map_err(|_| Error::ChangeFailed)?;
                    }
                    if session.uid_expunge(&uid).is_err() {
                        if !already_deleted {
                            let _ = session.uid_store(&uid, "-FLAGS.SILENT (\\Deleted)");
                        }
                        return Err(Error::ChangeFailed);
                    }
                }
                if let Some((cache, _)) = &cache {
                    let _ = std::fs::remove_file(cache.body_path(&id));
                }
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
        if let Some((cache, _lock)) = cache
            && let Some(mut snapshot) = cache.snapshot(folder)
        {
            change.apply(&mut snapshot.messages, &id);
            let _ = cache.save_snapshot(folder, &snapshot);
        }
        Ok(())
    })
    .await
}

pub async fn body(account: Account, id: MailId) -> Result<Arc<[u8]>, Error> {
    if account.microsoft_graph {
        return super::graph::body(account, id, MAX_BODY_BYTES).await;
    }
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

#[cfg(test)]
mod header_tests {
    use super::*;

    #[test]
    #[ignore = "requires loopback TCP"]
    fn cold_sync_publishes_recent_headers_before_requesting_older_mail() {
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
            time::Duration,
        };

        fn command(socket: &mut BufReader<TcpStream>, expected: &str) -> String {
            let mut line = String::new();
            socket.read_line(&mut line).unwrap();
            let (tag, request) = line.trim_end().split_once(' ').unwrap();
            assert_eq!(request, expected);
            tag.to_owned()
        }

        fn reply_with_headers(
            socket: &mut TcpStream,
            tag: &str,
            uids: std::ops::RangeInclusive<u32>,
        ) {
            for uid in uids {
                let bytes = format!(
                    "From: sender@example.org\r\nTo: inbox@example.org\r\nSubject: Message {uid}\r\n\r\n"
                );
                write!(socket, "* {uid} FETCH (UID {uid} FLAGS () RFC822.SIZE 123 INTERNALDATE \"24-Nov-2026 12:00:00 +0000\" BODY[HEADER] {{{}}}\r\n{bytes})\r\n", bytes.len()).unwrap();
            }
            write!(socket, "{tag} OK fetched\r\n").unwrap();
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (events, mut received) = mpsc::unbounded_channel();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut socket = BufReader::new(socket);
            writeln!(socket.get_mut(), "* OK ready\r").unwrap();
            let tag = command(&mut socket, "LOGIN \"test\" \"fixture\"");
            write!(socket.get_mut(), "{tag} OK logged in\r\n").unwrap();
            let tag = command(&mut socket, "EXAMINE \"INBOX\"");
            write!(
                socket.get_mut(),
                "* 52 EXISTS\r\n* OK [UIDVALIDITY 1] ready\r\n{tag} OK examined\r\n"
            )
            .unwrap();
            let tag = command(&mut socket, "UID SEARCH ALL");
            let uids = (1..=52)
                .map(|uid| uid.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            write!(socket.get_mut(), "* SEARCH {uids}\r\n{tag} OK searched\r\n").unwrap();

            let tag = command(
                &mut socket,
                "UID FETCH 3:52 (UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])",
            );
            reply_with_headers(socket.get_mut(), &tag, 3..=52);
            let tag = command(
                &mut socket,
                "UID FETCH 1:2 (UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])",
            );
            assert!(matches!(
                received.try_recv(),
                Ok(DownloadEvent::HeadersStarted(52))
            ));
            let Ok(DownloadEvent::HeadersReady(recent)) = received.try_recv() else {
                panic!("recent headers were not delivered before older mail was requested");
            };
            assert_eq!(recent.len(), 50);
            assert_eq!(recent.iter().map(|mail| mail.id.uid).max(), Some(52));
            reply_with_headers(socket.get_mut(), &tag, 1..=2);
            received
        });

        let socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut client = imap::Client::new(socket);
        client.read_greeting().unwrap();
        let mut session = client.login("test", "fixture").unwrap();
        let account = Account {
            id: uuid::Uuid::new_v4(),
            email: "inbox@example.org".into(),
            username: "test".into(),
            imap_host: "localhost".into(),
            imap_port: 993,
            online: false,
            microsoft_graph: false,
        };
        let (snapshot, _) = sync_headers(
            &mut session,
            &account,
            Folder::Inbox,
            None,
            None,
            &events,
            true,
        )
        .unwrap();
        assert_eq!(snapshot.messages.len(), 52);
        let mut received = server.join().unwrap();
        assert!(
            std::iter::from_fn(|| received.try_recv().ok()).any(|event| {
                matches!(event, DownloadEvent::HeadersReady(older) if older.len() == 2)
            })
        );
    }

    #[test]
    fn compresses_uid_ranges_without_fetching_missing_uids() {
        assert_eq!(uid_set([1, 2, 3, 7, 10, 11].into_iter()), "1:3,7,10:11");
        assert_eq!(uid_set([11, 10, 7, 3, 2, 1].into_iter()), "10:11,7,1:3");
        assert_eq!(
            uid_set([u32::MAX, u32::MAX - 1].into_iter()),
            "4294967294:4294967295"
        );
    }
}
