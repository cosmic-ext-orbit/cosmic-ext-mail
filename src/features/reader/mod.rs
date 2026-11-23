// SPDX-License-Identifier: GPL-3.0-only

use crate::features::mailbox::{Change, Contact, Mail, MailId, remote};
use cosmic::{Task, iced::Subscription};
use cosmic_ext_mail::EmailView;
use std::{collections::HashMap, sync::Arc};

mod attachments;
mod message;
mod view;

use attachments::Attachment;
pub use message::ComposeDetails;
use message::PreparedMessage;

#[derive(Clone, Debug)]
pub enum Message {
    Wake,
    BackToList,
    Retry,
    ShowRemoteImages,
    ConfirmPermanentDelete,
    SaveAttachment(u64, usize),
    AttachmentSaved(Result<(), ()>),
    SetTheme(MailId, Option<bool>),
    Loaded(u64, Result<PreparedMessage, remote::Error>),
    Reply,
    Forward,
    CopyAddress(String),
    ComposeTo(String),
    Change(Change),
}

pub struct Reader {
    email: Option<EmailView>,
    theme_overrides: HashMap<MailId, bool>,
    body: Body,
    request: u64,
    recipients: Vec<Contact>,
    attachments: Vec<Attachment>,
    attachment_source: Option<Arc<[u8]>>,
    forward_text: String,
    reply_address: Option<String>,
    message_id: Option<String>,
    open: bool,
}

enum Body {
    Empty,
    Loading,
    Ready,
    Failed(String),
}

impl Reader {
    pub fn new() -> Self {
        Self {
            email: None,
            theme_overrides: HashMap::new(),
            body: Body::Empty,
            request: 0,
            recipients: Vec::new(),
            attachments: Vec::new(),
            attachment_source: None,
            forward_text: String::new(),
            reply_address: None,
            message_id: None,
            open: false,
        }
    }

    pub fn release_window(&mut self) {
        self.request += 1;
        self.email = None;
        self.attachments.clear();
        self.attachment_source = None;
        self.body = Body::Empty;
    }

    pub fn load(
        &mut self,
        mail: Option<&Mail>,
        accounts: &crate::features::accounts::Accounts,
    ) -> Task<Message> {
        self.request += 1;
        self.body = Body::Empty;
        self.recipients.clear();
        self.attachments.clear();
        self.attachment_source = None;
        self.forward_text.clear();
        self.reply_address = None;
        self.message_id = None;
        let Some(mail) = mail else {
            return Task::none();
        };
        let Some(account) = accounts.saved().ok().and_then(|accounts| {
            accounts
                .iter()
                .find(|account| account.id == mail.id.account)
        }) else {
            self.body = Body::Failed(remote::Error::MessageUnavailable.text());
            return Task::none();
        };
        self.body = Body::Loading;
        let request = self.request;
        let account = account.clone();
        let id = mail.id.clone();
        Task::perform(
            async move {
                let source = remote::body(account, id).await?;
                tokio::task::spawn_blocking(move || PreparedMessage::parse(source))
                    .await
                    .map_err(|_| remote::Error::Fetch)
            },
            move |result| Message::Loaded(request, result),
        )
    }

    pub fn finish_load(
        &mut self,
        request: u64,
        result: Result<PreparedMessage, remote::Error>,
        load_remote_images: bool,
    ) {
        if request != self.request {
            return;
        }
        let rendered = result.map_err(remote::Error::text).and_then(|message| {
            self.reply_address = message.details.reply_address;
            self.message_id = message.details.message_id;
            self.forward_text = message.details.text;
            self.recipients = message.recipients;
            self.attachments = message.attachments;
            self.attachment_source = message.attachment_source;

            let body = message.body?;
            if let Some(view) = &mut self.email {
                view.set_prepared(body, load_remote_images);
            } else {
                self.email = Some(EmailView::from_prepared(
                    body,
                    load_remote_images,
                    cosmic::theme::is_dark(),
                )?);
            }
            Ok(())
        });
        self.body = match rendered {
            Ok(()) => Body::Ready,
            Err(error) => Body::Failed(error),
        };
    }

    pub fn set_remote_images(&mut self, enabled: bool) {
        if matches!(self.body, Body::Ready)
            && let Some(view) = &mut self.email
        {
            view.set_remote_images(enabled);
        }
    }

    pub fn set_theme(&mut self, id: MailId, dark: Option<bool>) {
        if let Some(dark) = dark {
            self.theme_overrides.insert(id, dark);
        } else {
            self.theme_overrides.remove(&id);
        }
    }

    pub fn reply_address<'a>(&'a self, mail: &'a Mail) -> &'a str {
        self.reply_address.as_deref().unwrap_or(&mail.address)
    }

    pub fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    pub fn forward_text(&self) -> &str {
        &self.forward_text
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.body, Body::Loading)
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub fn subscription(&self) -> Subscription<Message> {
        self.email.as_ref().map_or_else(Subscription::none, |view| {
            view.subscription().map(|()| Message::Wake)
        })
    }
}
