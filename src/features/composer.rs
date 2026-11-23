// SPDX-License-Identifier: GPL-3.0-only

mod delivery;
mod store;
mod view;

use crate::{
    features::{accounts::Account, mailbox::Mail},
    fl,
};
use chrono::Utc;
use cosmic::{
    Task,
    dialog::file_chooser,
    iced::{self, Subscription, widget::text_editor, window},
};
use lettre::message::Mailboxes;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;

use store::{ATTACHMENT_LIMIT, Store};

#[derive(Clone, Debug)]
pub enum Message {
    Open,
    OpenDraft(Uuid),
    Close,
    Autosave,
    SetFrom(usize),
    ToggleRecipients,
    AddressChanged(usize, String),
    AddAddress(usize),
    RemoveAddress(usize, usize),
    SubjectChanged(String),
    BodyChanged(text_editor::Action),
    Attach,
    Drop(Vec<PathBuf>),
    Attached(Uuid, Result<Vec<Attachment>, AttachError>),
    RemoveAttachment(Uuid),
    ConfirmDiscard(bool),
    Discard,
    Send,
    Retry,
    Sent(Uuid, Result<(), delivery::Error>),
}

#[derive(Clone, Copy, Debug)]
pub enum AttachError {
    TooLarge,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Attachment {
    id: Uuid,
    name: String,
    size: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
enum Delivery {
    #[default]
    Ready,
    Pending,
    Uncertain,
    Sent,
}

#[derive(Clone, Deserialize, Serialize)]
struct Draft {
    id: Uuid,
    account: Option<Uuid>,
    recipients: [Vec<String>; 3],
    inputs: [String; 3],
    subject: String,
    body: String,
    attachments: Vec<Attachment>,
    reply_to: Option<String>,
    delivery: Delivery,
    modified: i64,
}

impl Draft {
    fn new(account: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            account,
            recipients: Default::default(),
            inputs: Default::default(),
            subject: String::new(),
            body: String::new(),
            attachments: Vec::new(),
            reply_to: None,
            delivery: Delivery::Ready,
            modified: Utc::now().timestamp(),
        }
    }

    fn is_empty(&self) -> bool {
        self.subject.is_empty()
            && self.body.trim().is_empty()
            && self.attachments.is_empty()
            && self.inputs.iter().all(String::is_empty)
            && self.recipients.iter().all(Vec::is_empty)
    }
}

struct Session {
    draft: Draft,
    body: text_editor::Content<cosmic::Renderer>,
    extra_recipients: bool,
    invalid_address: Option<usize>,
    status: Option<String>,
    importing: bool,
    sending: bool,
    retrying: bool,
    confirm_discard: bool,
    dirty: bool,
}

impl Session {
    fn new(mut draft: Draft) -> Self {
        if draft.delivery == Delivery::Pending {
            draft.delivery = Delivery::Uncertain;
        }
        Self {
            dirty: !draft.is_empty(),
            body: text_editor::Content::with_text(&draft.body),
            extra_recipients: (1..3)
                .any(|i| !draft.recipients[i].is_empty() || !draft.inputs[i].is_empty()),
            draft,
            invalid_address: None,
            status: None,
            importing: false,
            sending: false,
            retrying: false,
            confirm_discard: false,
        }
    }

    fn busy(&self) -> bool {
        self.importing || self.sending
    }

    fn editable(&self) -> bool {
        !self.busy() && self.draft.delivery != Delivery::Sent && !self.confirm_discard
    }

    fn commit_addresses(&mut self, field: usize) -> bool {
        let Some(input) = self.draft.inputs.get_mut(field) else {
            return false;
        };
        let input_text = input.trim();
        if input_text.is_empty() {
            return true;
        }
        let Ok(addresses) = input_text.parse::<Mailboxes>() else {
            self.invalid_address = Some(field);
            return false;
        };
        for address in addresses.iter() {
            let address = address.to_string();
            if !self.draft.recipients[field].contains(&address) {
                self.draft.recipients[field].push(address);
            }
        }
        input.clear();
        self.invalid_address = None;
        self.dirty = true;
        true
    }
}

pub struct Composer {
    store: Option<Store>,
    drafts: Vec<Draft>,
    storage_error: bool,
    active: Option<Session>,
}

impl Default for Composer {
    fn default() -> Self {
        let store = Store::open().ok();
        let loaded = store.as_ref().and_then(|store| store.load().ok());
        let (drafts, storage_error) = loaded.unwrap_or((Vec::new(), true));
        Self {
            store,
            drafts: drafts
                .into_iter()
                .filter(|draft| draft.delivery != Delivery::Sent)
                .collect(),
            storage_error,
            active: None,
        }
    }
}

impl Composer {
    pub fn is_sending(&self) -> bool {
        self.active.as_ref().is_some_and(|session| session.sending)
    }

    pub fn is_open(&self) -> bool {
        self.active.is_some()
    }

    pub fn subscription(&self, main_window: Option<window::Id>) -> Subscription<Message> {
        if !self.is_open() {
            return Subscription::none();
        }
        Subscription::batch([
            iced::time::every(Duration::from_secs(2)).map(|_| Message::Autosave),
            window::events()
                .with(main_window)
                .filter_map(|(main, (id, event))| match event {
                    window::Event::FileDropped(paths) if main == Some(id) => {
                        Some(Message::Drop(paths))
                    }
                    _ => None,
                }),
        ])
    }

    pub fn compose_to(&mut self, address: String, account: Option<Uuid>) -> Task<Message> {
        let mut draft = Draft::new(account);
        draft.inputs[0] = address;
        self.open(draft)
    }

    pub fn reply(
        &mut self,
        mail: &Mail,
        address: &str,
        reply_to: Option<&str>,
        text: &str,
    ) -> Task<Message> {
        let mut draft = Draft::new(Some(mail.id.account));
        draft.inputs[0] = address.to_owned();
        draft.subject = if mail
            .subject
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
        {
            mail.subject.clone()
        } else {
            format!("Re: {}", mail.subject)
        };
        draft.reply_to = reply_to
            .map(|id| id.trim_matches(['<', '>']))
            .filter(|id| !id.is_empty() && !id.contains(['\r', '\n', '<', '>']))
            .map(|id| format!("<{id}>"));
        if !text.is_empty() {
            let quoted = text
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            draft.body = format!(
                "\n\n{}\n{quoted}",
                fl!("reply-attribution", sender = mail.sender.as_str())
            );
        }
        self.open(draft)
    }

    pub fn forward(&mut self, mail: &Mail, text: &str) -> Task<Message> {
        let mut draft = Draft::new(Some(mail.id.account));
        draft.subject = fl!("forward-subject", subject = mail.subject.as_str());
        draft.body = fl!(
            "forwarded-message",
            sender = mail.sender.as_str(),
            subject = mail.subject.as_str(),
            date = mail.time.as_str(),
            body = text,
        );
        self.open(draft)
    }

    fn open(&mut self, draft: Draft) -> Task<Message> {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.busy() || session.draft.id == draft.id)
            || !self.persist()
        {
            return Task::none();
        }
        self.active = Some(Session::new(draft));
        Task::none()
    }

    pub fn persist(&mut self) -> bool {
        let Some(session) = &mut self.active else {
            return true;
        };
        if !session.dirty {
            return true;
        }
        session.draft.body = session.body.text();
        if session.draft.is_empty() && !self.drafts.iter().any(|draft| draft.id == session.draft.id)
        {
            session.dirty = false;
            return true;
        }
        session.draft.modified = Utc::now().timestamp();
        if self
            .store
            .as_ref()
            .is_none_or(|store| store.save(&session.draft).is_err())
        {
            session.status = Some(fl!("draft-storage-error"));
            return false;
        }
        session.dirty = false;
        self.drafts.retain(|draft| draft.id != session.draft.id);
        if session.draft.delivery != Delivery::Sent {
            self.drafts.insert(0, session.draft.clone());
        }
        true
    }

    pub fn prepare_to_exit(&mut self) -> bool {
        !self.active.as_ref().is_some_and(Session::busy) && self.persist()
    }

    fn close(&mut self) -> Task<Message> {
        if let Some(session) = self.active.take()
            && session.draft.delivery == Delivery::Sent
            && let Some(store) = &self.store
        {
            let _ = store.remove(session.draft.id);
        }
        Task::none()
    }

    pub fn update(&mut self, message: Message, accounts: &[Account]) -> Task<Message> {
        match message {
            Message::Open => {
                return self.open(Draft::new(accounts.first().map(|account| account.id)));
            }
            Message::OpenDraft(id) => {
                if let Some(draft) = self.drafts.iter().find(|draft| draft.id == id).cloned() {
                    return self.open(draft);
                }
            }
            Message::Autosave => {
                self.persist();
            }
            Message::Close => {
                if !self.active.as_ref().is_some_and(Session::busy) && self.persist() {
                    return self.close();
                }
            }
            Message::Drop(paths) => return self.attach(Some(paths)),
            Message::Attach => return self.attach(None),
            Message::Sent(id, result) => return self.sent(id, result),
            Message::Send | Message::Retry => {
                return self.send(accounts, matches!(message, Message::Retry));
            }
            message => {
                let Some(session) = &mut self.active else {
                    return Task::none();
                };
                if let Message::Attached(id, result) = message {
                    if session.draft.id == id {
                        session.importing = false;
                        match result {
                            Ok(files) => {
                                session.draft.attachments.extend(files);
                                session.dirty = true;
                                self.persist();
                            }
                            Err(AttachError::TooLarge) => {
                                session.status = Some(fl!("attachment-too-large"))
                            }
                            Err(AttachError::Failed) => {
                                session.status = Some(fl!("attachment-failed"))
                            }
                        }
                    }
                    return Task::none();
                }
                if session.busy() {
                    return Task::none();
                }
                match message {
                    Message::ConfirmDiscard(confirm) => session.confirm_discard = confirm,
                    Message::Discard if session.confirm_discard => {
                        let id = session.draft.id;
                        if self
                            .store
                            .as_ref()
                            .is_some_and(|store| store.remove(id).is_err())
                        {
                            session.status = Some(fl!("draft-storage-error"));
                        } else {
                            self.drafts.retain(|draft| draft.id != id);
                            return self.close();
                        }
                    }
                    _ if !session.editable() => (),
                    Message::SetFrom(index) => {
                        if let Some(account) = accounts.get(index) {
                            session.draft.account = Some(account.id);
                            session.dirty = true;
                        }
                    }
                    Message::ToggleRecipients => {
                        session.extra_recipients = !session.extra_recipients
                    }
                    Message::AddressChanged(field, text) => {
                        if let Some(input) = session.draft.inputs.get_mut(field) {
                            *input = text;
                            session.invalid_address = None;
                            session.dirty = true;
                        }
                    }
                    Message::AddAddress(field) => {
                        session.commit_addresses(field);
                    }
                    Message::RemoveAddress(field, index) => {
                        if let Some(addresses) = session.draft.recipients.get_mut(field)
                            && index < addresses.len()
                        {
                            addresses.remove(index);
                            session.dirty = true;
                        }
                    }
                    Message::SubjectChanged(subject) => {
                        session.draft.subject = subject;
                        session.dirty = true;
                    }
                    Message::BodyChanged(action) => {
                        session.dirty |= action.is_edit();
                        session.body.perform(action);
                    }
                    Message::RemoveAttachment(id) => {
                        session.draft.attachments.retain(|file| file.id != id);
                        session.dirty = true;
                        let draft_id = session.draft.id;
                        if self.persist()
                            && let Some(store) = &self.store
                        {
                            let _ = store.remove_attachment(draft_id, id);
                        }
                    }
                    _ => (),
                }
            }
        }
        Task::none()
    }

    fn attach(&mut self, paths: Option<Vec<PathBuf>>) -> Task<Message> {
        let Some(session) = self.active.as_mut().filter(|session| session.editable()) else {
            return Task::none();
        };
        let Some(store) = self.store.clone() else {
            session.status = Some(fl!("draft-storage-error"));
            return Task::none();
        };
        let id = session.draft.id;
        let budget = ATTACHMENT_LIMIT
            .saturating_sub(session.draft.attachments.iter().map(|file| file.size).sum());
        session.importing = true;
        session.status = None;
        Task::perform(
            async move {
                let paths = match paths {
                    Some(paths) => paths,
                    None => match file_chooser::open::Dialog::new()
                        .title(fl!("attach-files"))
                        .open_files()
                        .await
                    {
                        Ok(response) => response
                            .urls()
                            .iter()
                            .map(|url| url.to_file_path().map_err(|_| AttachError::Failed))
                            .collect::<Result<Vec<_>, _>>()?,
                        Err(file_chooser::Error::Cancelled) => return Ok(Vec::new()),
                        Err(_) => return Err(AttachError::Failed),
                    },
                };
                tokio::task::spawn_blocking(move || store.import(id, paths, budget))
                    .await
                    .map_err(|_| AttachError::Failed)?
                    .map_err(|error| {
                        if error.kind() == std::io::ErrorKind::FileTooLarge {
                            AttachError::TooLarge
                        } else {
                            AttachError::Failed
                        }
                    })
            },
            move |result| Message::Attached(id, result),
        )
    }

    fn send(&mut self, accounts: &[Account], retry: bool) -> Task<Message> {
        let Some(session) = self.active.as_mut().filter(|session| session.editable()) else {
            return Task::none();
        };
        if (session.draft.delivery == Delivery::Uncertain) != retry {
            return Task::none();
        }
        session.status = None;
        if !(0..3).all(|field| session.commit_addresses(field)) {
            session.extra_recipients |= session.invalid_address.is_some_and(|field| field != 0);
            return Task::none();
        }
        if session.draft.recipients.iter().all(Vec::is_empty) {
            session.status = Some(fl!("compose-add-recipient"));
            return Task::none();
        }
        let Some(account) = accounts
            .iter()
            .find(|account| Some(account.id) == session.draft.account)
            .cloned()
        else {
            session.status = Some(fl!("compose-select-account"));
            return Task::none();
        };
        let Some(store) = self.store.clone() else {
            session.status = Some(fl!("draft-storage-error"));
            return Task::none();
        };
        let previous = session.draft.delivery;
        session.draft.delivery = Delivery::Pending;
        session.dirty = true;
        if !self.persist() {
            if let Some(session) = &mut self.active {
                session.draft.delivery = previous;
            }
            return Task::none();
        }
        let session = self.active.as_mut().expect("open composer");
        session.sending = true;
        session.retrying = retry;
        let draft = session.draft.clone();
        let id = draft.id;
        Task::perform(delivery::send(store, draft, account), move |result| {
            Message::Sent(id, result)
        })
    }

    fn sent(&mut self, id: Uuid, result: Result<(), delivery::Error>) -> Task<Message> {
        let Some(session) = self
            .active
            .as_mut()
            .filter(|session| session.draft.id == id)
        else {
            return Task::none();
        };
        session.sending = false;
        session.draft.delivery = match result {
            Ok(()) => Delivery::Sent,
            Err(delivery::Error::Uncertain) => Delivery::Uncertain,
            Err(_) if session.retrying => Delivery::Uncertain,
            Err(_) => Delivery::Ready,
        };
        session.status = match result {
            Ok(()) => Some(fl!("message-sent")),
            Err(delivery::Error::Account(error)) => Some(error.text()),
            Err(delivery::Error::Attachment) => Some(fl!("attachment-failed")),
            Err(delivery::Error::Message) => Some(fl!("compose-message-error")),
            Err(delivery::Error::Rejected) => Some(fl!("smtp-rejected")),
            Err(delivery::Error::Uncertain) => None,
        };
        session.dirty = true;
        let saved = self.persist();
        if result.is_ok() {
            if saved {
                return self.close();
            }
            if let Some(session) = &mut self.active {
                session.status = Some(fl!("sent-draft-storage-error"));
            }
        }
        Task::none()
    }
}
