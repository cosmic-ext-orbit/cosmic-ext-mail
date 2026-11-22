// SPDX-License-Identifier: GPL-3.0-only

use crate::{
    config::MessageActions,
    features::mailbox::{Folder, Mail, remote},
    fl,
};
use cosmic::{Element, widget};
use cosmic::{
    Task,
    iced::{Length, Subscription},
};
use cosmic_ext_mail::{EmailBody, EmailView};
use mail_parser::MessageParser;
use std::sync::Arc;

const INLINE_ACTIONS_MIN_WIDTH: f32 = 520.0;

#[derive(Clone, Debug)]
pub enum Message {
    Wake,
    BackToList,
    Retry,
    ShowRemoteImages,
    Loaded(u64, Result<Arc<[u8]>, remote::Error>),
    Reply,
    Forward,
    ToggleMore,
    Change(remote::Change),
    Changed(Folder, u64, Result<(), remote::Error>),
}

pub struct Reader {
    email: Option<EmailView>,
    body: Body,
    request: u64,
    recipients: String,
    forward_text: String,
    more: bool,
    changing: bool,
    action_error: Option<String>,
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
            recipients: String::new(),
            forward_text: String::new(),
            more: false,
            changing: false,
            action_error: None,
            open: false,
        }
    }

    pub fn load(
        &mut self,
        mail: Option<&Mail>,
        accounts: &crate::features::accounts::Accounts,
    ) -> Task<Message> {
        self.request += 1;
        self.body = Body::Empty;
        self.recipients.clear();
        self.forward_text.clear();
        self.more = false;
        self.action_error = None;
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
        Task::perform(
            remote::body(account.clone(), mail.id.clone()),
            move |result| Message::Loaded(request, result),
        )
    }

    pub fn finish_load(
        &mut self,
        request: u64,
        result: Result<Arc<[u8]>, remote::Error>,
        load_remote_images: bool,
    ) {
        if request != self.request {
            return;
        }
        let rendered = result.map_err(remote::Error::text).and_then(|bytes| {
            let message = MessageParser::new().parse(&bytes);
            if let Some(addresses) = message.as_ref().and_then(|message| message.to()) {
                self.recipients = addresses
                    .mailboxes()
                    .filter_map(|recipient| {
                        let address = recipient.address().filter(|address| !address.is_empty());
                        let name = recipient.name().filter(|name| !name.is_empty());
                        match (name, address) {
                            (Some(name), Some(address)) if name != address => {
                                Some(format!("{name} <{address}>"))
                            }
                            (name, address) => address.or(name).map(str::to_owned),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
            }

            if let Some(text) = message.as_ref().and_then(|message| message.body_text(0)) {
                self.forward_text = text.into_owned();
            }
            let body = EmailBody::Mime(&bytes);
            if let Some(view) = &mut self.email {
                view.set_body_with_remote_images(body, load_remote_images)?;
            } else {
                self.email = Some(EmailView::new_with_remote_images(body, load_remote_images)?);
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

    pub fn forward_text(&self) -> &str {
        &self.forward_text
    }

    pub fn set_action_error(&mut self, error: String) {
        self.action_error = Some(error);
    }

    pub fn toggle_more(&mut self) {
        self.more = !self.more;
    }

    pub fn begin_change(&mut self) -> Option<u64> {
        if self.changing {
            return None;
        }
        self.changing = true;
        self.action_error = None;
        Some(self.request)
    }

    pub fn finish_change(&mut self, request: u64, result: Result<(), remote::Error>) {
        self.changing = false;
        if self.request == request {
            self.action_error = result.err().map(remote::Error::text);
        }
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

    pub fn view<'a>(
        &'a self,
        mail: Option<&'a Mail>,
        folder: Folder,
        narrow: bool,
        style: MessageActions,
    ) -> Element<'a, Message> {
        let mut page = widget::column::with_capacity(4)
            .spacing(8)
            .padding(12)
            .height(Length::Fill);
        if narrow {
            page =
                page.push(widget::button::text(fl!("back-to-list")).on_press(Message::BackToList));
        }

        let Some(mail) = mail else {
            return page.push(widget::text::body(fl!("select-message"))).into();
        };
        let body: Element<_> = match &self.body {
            Body::Ready => self.email.as_ref().expect("ready email view").view(),
            Body::Failed(error) => widget::column::with_capacity(2)
                .push(widget::text(fl!("preview-failed", error = error.as_str())))
                .push(widget::button::text(fl!("mail-retry")).on_press(Message::Retry))
                .into(),
            Body::Empty | Body::Loading => widget::text::body(fl!("mail-body-loading")).into(),
        };
        page.push(
            widget::responsive(move |size| self.header(mail, folder, style, size.width))
                .height(Length::Shrink),
        )
        .push(widget::divider::horizontal::default())
        .push(body)
        .into()
    }

    fn header<'a>(
        &'a self,
        mail: &'a Mail,
        folder: Folder,
        style: MessageActions,
        width: f32,
    ) -> Element<'a, Message> {
        let mut sender = widget::column::with_capacity(2)
            .push(widget::text::heading(mail.sender.as_str()))
            .width(Length::Fill)
            .spacing(2);
        if !mail.address.is_empty() && mail.address != mail.sender {
            sender = sender.push(widget::text::body(mail.address.as_str()));
        }

        let change = |action| (!self.changing).then_some(Message::Change(action));
        let move_to = |destination| {
            (folder != destination)
                .then(|| change(remote::Change::Move(destination)))
                .flatten()
        };
        let actions = widget::row::with_children([
            action_button(
                style,
                "mail-reply-all-symbolic",
                fl!("reply"),
                (!mail.address.is_empty()).then_some(Message::Reply),
            ),
            action_button(
                style,
                "mail-forward-symbolic",
                fl!("forward"),
                matches!(self.body, Body::Ready).then_some(Message::Forward),
            ),
            action_button(
                style,
                "mail-archive-symbolic",
                fl!("archive-message"),
                move_to(Folder::Archive),
            ),
            action_button(
                style,
                "mail-mark-junk-symbolic",
                fl!("spam-message"),
                move_to(Folder::Junk),
            ),
            action_button(
                style,
                "user-trash-symbolic",
                fl!("delete-message"),
                move_to(Folder::Trash),
            ),
            action_button(
                style,
                "view-more-symbolic",
                fl!("more-message-actions"),
                Some(Message::ToggleMore),
            ),
            action_button(
                style,
                if mail.flagged {
                    "mail-mark-important-symbolic"
                } else {
                    "insert-star-symbolic"
                },
                if mail.flagged {
                    fl!("unstar-message")
                } else {
                    fl!("star-message")
                },
                change(remote::Change::Flagged(!mail.flagged)),
            ),
        ])
        .spacing(4)
        .wrap();

        let mut metadata =
            widget::row::with_capacity(2)
                .spacing(8)
                .push(if self.recipients.is_empty() {
                    Element::from(widget::space::horizontal())
                } else {
                    widget::text::caption(fl!("mail-to", recipients = self.recipients.as_str()))
                        .width(Length::Fill)
                        .into()
                });
        if !mail.time.is_empty() {
            metadata = metadata.push(widget::text::caption(mail.time.as_str()));
        }

        let mut header = widget::column::with_capacity(4).spacing(8);
        let inline_width = match style {
            MessageActions::IconOnly => INLINE_ACTIONS_MIN_WIDTH,
            MessageActions::TextOnly => 675.0,
            MessageActions::IconAndText => 825.0,
        };
        let inline_actions = width >= inline_width;
        let trailing_actions = if inline_actions {
            header = header.push(
                widget::row::with_capacity(2)
                    .spacing(12)
                    .push(sender)
                    .push(actions),
            );
            None
        } else {
            header = header.push(sender);
            Some(actions)
        };
        header = header
            .push(metadata)
            .push(widget::text::title2(mail.subject.as_str()));
        if let Some(actions) = trailing_actions {
            header = header.push(actions);
        }
        if self.more {
            let (icon, label) = if mail.unread {
                ("mail-mark-read-symbolic", fl!("mark-message-read"))
            } else {
                ("mail-mark-unread-symbolic", fl!("mark-message-unread"))
            };
            let mut more = widget::row::with_capacity(2).spacing(4).push(action_button(
                style,
                icon,
                label,
                change(remote::Change::Seen(mail.unread)),
            ));
            if matches!(self.body, Body::Ready)
                && self
                    .email
                    .as_ref()
                    .is_some_and(|view| !view.remote_images_enabled())
            {
                more = more.push(action_button(
                    style,
                    "insert-image-symbolic",
                    fl!("mail-show-remote-images"),
                    Some(Message::ShowRemoteImages),
                ));
            }
            header = header.push(more.wrap());
        }
        if self.changing {
            header = header.push(widget::text::caption(fl!("mail-change-working")));
        } else if let Some(error) = &self.action_error {
            header = header.push(widget::text::caption(error.as_str()));
        }
        header.into()
    }
}

fn action_button(
    style: MessageActions,
    icon: &'static str,
    label: String,
    message: Option<Message>,
) -> Element<'static, Message> {
    match style {
        MessageActions::IconOnly => widget::button::icon(widget::icon::from_name(icon))
            .tooltip(label)
            .on_press_maybe(message)
            .into(),
        MessageActions::TextOnly => widget::button::standard(label)
            .on_press_maybe(message)
            .into(),
        MessageActions::IconAndText => widget::button::standard(label)
            .leading_icon(widget::icon::from_name(icon))
            .on_press_maybe(message)
            .into(),
    }
}
