// SPDX-License-Identifier: GPL-3.0-only

use crate::{
    features::mailbox::{Mail, remote},
    fl,
};
use cosmic::iced::{Length, Subscription};
use cosmic::{Element, widget};
use cosmic_ext_mail::{EmailBody, EmailView};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum ReaderMessage {
    Wake,
    BackToList,
    Retry,
    ShowRemoteImages,
    Loaded(u64, Result<Arc<[u8]>, remote::Error>),
    Reply,
}

pub struct Reader {
    email: Option<EmailView>,
    body: Body,
    request: u64,
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
            body: Body::Empty,
            request: 0,
            open: false,
        }
    }

    pub fn load(
        &mut self,
        mail: Option<&Mail>,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<ReaderMessage> {
        self.request += 1;
        self.body = Body::Empty;
        let Some(mail) = mail else {
            return cosmic::Task::none();
        };
        let Some(account) = accounts.saved().ok().and_then(|accounts| {
            accounts
                .iter()
                .find(|account| account.id == mail.id.account)
        }) else {
            self.body = Body::Failed(remote::Error::MessageUnavailable.text());
            return cosmic::Task::none();
        };
        self.body = Body::Loading;
        let request = self.request;
        cosmic::Task::perform(
            remote::body(account.clone(), mail.id.clone()),
            move |result| ReaderMessage::Loaded(request, result),
        )
    }

    pub fn finish_load(&mut self, request: u64, result: Result<Arc<[u8]>, remote::Error>) {
        if request != self.request {
            return;
        }
        let rendered = result.map_err(remote::Error::text).and_then(|bytes| {
            let body = EmailBody::Mime(&bytes);
            if let Some(view) = &mut self.email {
                view.set_body(body)
            } else {
                self.email = Some(EmailView::new(body)?);
                Ok(())
            }
        });
        self.body = match rendered {
            Ok(()) => Body::Ready,
            Err(error) => Body::Failed(error),
        };
    }

    pub fn show_remote_images(&mut self) {
        if matches!(self.body, Body::Ready)
            && let Some(view) = &mut self.email
        {
            view.show_remote_images();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub fn subscription(&self) -> Subscription<ReaderMessage> {
        self.email.as_ref().map_or_else(Subscription::none, |view| {
            view.subscription().map(|()| ReaderMessage::Wake)
        })
    }

    pub fn view<'a>(&'a self, mail: Option<&'a Mail>, narrow: bool) -> Element<'a, ReaderMessage> {
        let mut page = widget::column::with_capacity(5)
            .spacing(8)
            .padding(12)
            .height(Length::Fill);
        if narrow {
            page = page.push(
                widget::button::text(fl!("back-to-list")).on_press(ReaderMessage::BackToList),
            );
        }

        let Some(mail) = mail else {
            return page.push(widget::text::body(fl!("select-message"))).into();
        };
        let body: Element<_> = match &self.body {
            Body::Ready => self.email.as_ref().expect("ready email view").view(),
            Body::Failed(error) => widget::column::with_capacity(2)
                .push(widget::text(fl!("preview-failed", error = error.as_str())))
                .push(widget::button::text(fl!("mail-retry")).on_press(ReaderMessage::Retry))
                .into(),
            Body::Empty | Body::Loading => widget::text::body(fl!("mail-body-loading")).into(),
        };
        let sender = if mail.address.is_empty() || mail.sender == mail.address {
            mail.sender.clone()
        } else {
            format!("{} <{}>", mail.sender, mail.address)
        };
        let mut actions = widget::row::with_capacity(2).spacing(8).push(
            widget::button::standard(fl!("reply"))
                .on_press_maybe((!mail.address.is_empty()).then_some(ReaderMessage::Reply)),
        );
        if matches!(self.body, Body::Ready)
            && self
                .email
                .as_ref()
                .is_some_and(|view| !view.remote_images_enabled())
        {
            actions = actions.push(
                widget::button::text(fl!("mail-show-remote-images"))
                    .on_press(ReaderMessage::ShowRemoteImages),
            );
        }
        page = page
            .push(actions.wrap())
            .push(widget::text::title2(mail.subject.as_str()))
            .push(widget::text::body(sender))
            .push(widget::text::caption(mail.time.as_str()));
        page.push(body).into()
    }
}
