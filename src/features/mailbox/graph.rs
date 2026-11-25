// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashSet, sync::Arc};

use chrono::DateTime;
use io_msgraph::v1::{
    client::{MsgraphClientStd, MsgraphClientStdError},
    rest::users::messages::{
        MsgraphFlagStatus, MsgraphFollowupFlag, MsgraphMessage, list::MsgraphMessagesListParams,
    },
};
use mail_parser::MessageParser;
use tokio::sync::mpsc;

use super::{
    Change, Contact, Folder, Mail, MailId,
    cache::{self, Cache, Snapshot},
    remote::{DownloadEvent, Error},
    threads::ThreadHeaders,
};
use crate::{
    features::accounts::{self, Account},
    fl,
};

pub(crate) enum GraphError {
    Account(accounts::Error),
    Request(MsgraphClientStdError),
    Cancelled,
    InvalidPage,
    WrongFolder,
}

impl From<MsgraphClientStdError> for GraphError {
    fn from(error: MsgraphClientStdError) -> Self {
        Self::Request(error)
    }
}

impl GraphError {
    pub(crate) fn status(&self) -> Option<u16> {
        match self {
            Self::Request(MsgraphClientStdError::Send(error)) => error.status(),
            _ => None,
        }
    }

    fn mailbox_error(self, fallback: Error) -> Error {
        match self {
            Self::Account(error) => error.into(),
            Self::WrongFolder => Error::MessageUnavailable,
            error => match error.status() {
                Some(401 | 403) => accounts::Error::OnlineReconnect.into(),
                Some(404) => Error::MessageUnavailable,
                _ => fallback,
            },
        }
    }
}

pub(crate) async fn with_client<T, F>(account_id: uuid::Uuid, operation: F) -> Result<T, GraphError>
where
    T: Send + 'static,
    F: Fn(&mut MsgraphClientStd) -> Result<T, GraphError> + Send + Sync + 'static,
{
    let operation = Arc::new(operation);
    for refresh in [false, true] {
        let token = accounts::graph_token(account_id, refresh)
            .await
            .map_err(GraphError::Account)?;
        let operation = Arc::clone(&operation);
        let result = tokio::task::spawn_blocking(move || {
            let mut client = MsgraphClientStd::connect(token.0, Default::default())?;
            operation(&mut client)
        })
        .await
        .map_err(|_| GraphError::Account(accounts::Error::ConnectionTask))?;
        if !refresh && result.as_ref().err().and_then(GraphError::status) == Some(401) {
            continue;
        }
        return result;
    }
    unreachable!("the final token attempt returns its result")
}

fn folder_name(folder: Folder) -> &'static str {
    match folder {
        Folder::Inbox => "inbox",
        Folder::Drafts => "drafts",
        Folder::Sent => "sentitems",
        Folder::Archive => "archive",
        Folder::Junk => "junkemail",
        Folder::Trash => "deleteditems",
    }
}

pub(super) async fn sync(
    account: Account,
    folder: Folder,
    downloads: mpsc::UnboundedSender<DownloadEvent>,
    stream_headers: bool,
) -> Result<Snapshot, Error> {
    let account_id = account.id;
    let (snapshot, new_count) = with_client(account_id, move |client| {
        if downloads.is_closed() {
            return Err(GraphError::Cancelled);
        }

        let cache = Cache::for_account(&account);
        let lock = cache
            .as_ref()
            .and_then(|cache| cache.sync_lock(folder).ok());
        let cache = cache.filter(|_| lock.is_some());
        let previous = cache.as_ref().and_then(|cache| cache.snapshot(folder));
        let folder_name = folder_name(folder);
        let params = MsgraphMessagesListParams {
            top: Some(100),
            select: Some(concat!(
                "id,subject,from,sender,toRecipients,receivedDateTime,sentDateTime,",
                "isRead,flag,internetMessageId,internetMessageHeaders"
            )),
            orderby: Some("receivedDateTime desc"),
            ..Default::default()
        };

        let mut page = client.messages_list(Some(folder_name), &params)?.response;
        let mut messages = Vec::new();
        let mut seen_messages = HashSet::new();
        let mut seen_links = HashSet::new();
        loop {
            if downloads.is_closed() {
                return Err(GraphError::Cancelled);
            }
            let batch: Vec<_> = page
                .value
                .into_iter()
                .filter(|message| {
                    !message.id.is_empty() && seen_messages.insert(message.id.clone())
                })
                .map(|message| into_mail(&account, folder_name, message))
                .collect();
            if stream_headers && !batch.is_empty() {
                let _ = downloads.send(DownloadEvent::HeadersReady(batch.clone()));
            }
            messages.extend(batch);
            let Some(next) = page.next_link else { break };
            if !seen_links.insert(next.clone()) {
                return Err(GraphError::InvalidPage);
            }
            page = client.page_get(&next, Some(100))?.response;
        }

        let snapshot = Snapshot {
            messages,
            uid_validity: None,
        };
        let new_count = if folder == Folder::Inbox {
            previous.map_or(0, |previous| {
                let known: HashSet<_> = previous.messages.iter().map(|mail| &mail.id).collect();
                snapshot
                    .messages
                    .iter()
                    .filter(|mail| mail.unread && !known.contains(&mail.id))
                    .count()
            })
        } else {
            0
        };
        if let Some(cache) = cache {
            let _ = cache.save_snapshot(folder, &snapshot);
        }
        Ok((snapshot, new_count))
    })
    .await
    .map_err(|error| error.mailbox_error(Error::Fetch))?;

    if new_count > 0 {
        tokio::spawn(super::notifications::new_mail(account_id, new_count));
    }
    Ok(snapshot)
}

fn into_mail(account: &Account, folder: &str, message: MsgraphMessage) -> Mail {
    let from = message
        .from
        .or(message.sender)
        .map(|recipient| recipient.email_address);
    let address = from
        .as_ref()
        .and_then(|from| from.address.as_deref())
        .unwrap_or_default();
    let sender = from
        .as_ref()
        .and_then(|from| from.name.as_deref())
        .filter(|name| !name.is_empty())
        .or_else(|| (!address.is_empty()).then_some(address))
        .map_or_else(|| fl!("mail-unknown-sender"), str::to_owned);
    let date = message
        .received_date_time
        .or(message.sent_date_time)
        .as_deref()
        .and_then(|date| DateTime::parse_from_rfc3339(date).ok());

    let mut raw_headers = Vec::new();
    for header in message.internet_message_headers {
        if ["message-id", "references", "in-reply-to"]
            .iter()
            .any(|name| header.name.eq_ignore_ascii_case(name))
        {
            raw_headers.extend_from_slice(header.name.as_bytes());
            raw_headers.extend_from_slice(b": ");
            raw_headers.extend_from_slice(header.value.as_bytes());
            raw_headers.extend_from_slice(b"\r\n");
        }
    }
    raw_headers.extend_from_slice(b"\r\n");
    let mut thread = MessageParser::default()
        .parse_headers(&raw_headers)
        .as_ref()
        .map_or_else(ThreadHeaders::default, ThreadHeaders::from_headers);
    if thread.message_id.is_none() {
        thread.message_id = message
            .internet_message_id
            .map(|id| id.trim_matches(['<', '>']).to_owned());
    }

    Mail {
        id: MailId {
            account: account.id,
            folder: folder.into(),
            uid_validity: 0,
            uid: 0,
            graph_id: Some(message.id),
        },
        account_email: account.email.clone(),
        unread: message.is_read == Some(false),
        flagged: message.flag.and_then(|flag| flag.flag_status) == Some(MsgraphFlagStatus::Flagged),
        sender,
        address: address.to_owned(),
        recipients: Some(
            message
                .to_recipients
                .into_option()
                .unwrap_or_default()
                .into_iter()
                .map(|recipient| Contact {
                    name: recipient.email_address.name.unwrap_or_default(),
                    address: recipient.email_address.address.unwrap_or_default(),
                })
                .collect(),
        ),
        subject: message
            .subject
            .filter(|subject| !subject.is_empty())
            .unwrap_or_else(|| fl!("mail-no-subject")),
        timestamp: date.map_or(i64::MIN, |date| date.timestamp()),
        time: date
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        thread: Some(thread),
    }
}

pub(super) async fn body(account: Account, id: MailId, limit: usize) -> Result<Arc<[u8]>, Error> {
    let graph_id = id.graph_id.clone().ok_or(Error::MessageUnavailable)?;
    let path = Cache::for_account(&account).map(|cache| cache.body_path(&id));
    if let Some(path) = &path
        && let Ok(bytes) = tokio::fs::read(path).await
        && bytes.len() <= limit
    {
        return Ok(bytes.into());
    }

    let bytes = with_client(account.id, move |client| {
        Ok(client.message_get_raw(&graph_id)?.response)
    })
    .await
    .map_err(|error| error.mailbox_error(Error::Fetch))?;
    if bytes.len() > limit {
        return Err(Error::MessageTooLarge);
    }
    let bytes: Arc<[u8]> = bytes.into();
    if let Some(path) = path {
        let body = bytes.clone();
        let _ = tokio::task::spawn_blocking(move || cache::save_body(&path, &body)).await;
    }
    Ok(bytes)
}

pub(super) async fn change(
    account: Account,
    folder: Folder,
    id: MailId,
    change: Change,
) -> Result<(), Error> {
    let graph_id = id.graph_id.clone().ok_or(Error::MessageUnavailable)?;
    with_client(account.id, move |client| {
        let cache = Cache::for_account(&account);
        let lock = cache
            .as_ref()
            .and_then(|cache| cache.sync_lock(folder).ok());
        let cache = cache.filter(|_| lock.is_some());
        match change {
            Change::Move(destination) => {
                client.message_move(&graph_id, folder_name(destination))?;
            }
            Change::DeletePermanently => {
                if folder != Folder::Trash {
                    return Err(GraphError::WrongFolder);
                }
                let trash_id = client
                    .mail_folder_get(folder_name(Folder::Trash))?
                    .response
                    .id;
                let current = client
                    .message_get(&graph_id, Some("parentFolderId"), None)?
                    .response;
                if current.parent_folder_id.as_deref() != Some(trash_id.as_str()) {
                    return Err(GraphError::WrongFolder);
                }
                client.message_permanent_delete(&graph_id)?;
                if let Some(cache) = &cache {
                    let _ = std::fs::remove_file(cache.body_path(&id));
                }
            }
            Change::Seen(read) => {
                client.message_update(
                    &graph_id,
                    &MsgraphMessage {
                        is_read: Some(read),
                        ..Default::default()
                    },
                )?;
            }
            Change::Flagged(flagged) => {
                let flag_status = if flagged {
                    MsgraphFlagStatus::Flagged
                } else {
                    MsgraphFlagStatus::NotFlagged
                };
                client.message_update(
                    &graph_id,
                    &MsgraphMessage {
                        flag: Some(MsgraphFollowupFlag {
                            flag_status: Some(flag_status),
                        }),
                        ..Default::default()
                    },
                )?;
            }
        }
        if let Some(cache) = cache
            && let Some(mut snapshot) = cache.snapshot(folder)
        {
            change.apply(&mut snapshot.messages, &id);
            let _ = cache.save_snapshot(folder, &snapshot);
        }
        Ok(())
    })
    .await
    .map_err(|error| error.mailbox_error(Error::ChangeFailed))
}
