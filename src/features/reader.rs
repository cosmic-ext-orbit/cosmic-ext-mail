// SPDX-License-Identifier: GPL-3.0-only

use crate::{
    config::{Config, MessageActions},
    features::mailbox::{Contact, Folder, Mail, remote},
    fl,
};
use cosmic::widget::menu;
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
    CopyAddress(String),
    ComposeTo(String),
    Change(remote::Change),
    Changed(Folder, u64, Result<(), remote::Error>),
}

pub struct Reader {
    email: Option<EmailView>,
    body: Body,
    request: u64,
    recipients: Vec<Contact>,
    forward_text: String,
    reply_address: Option<String>,
    message_id: Option<String>,
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
            recipients: Vec::new(),
            forward_text: String::new(),
            reply_address: None,
            message_id: None,
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
        self.reply_address = None;
        self.message_id = None;
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
            self.reply_address = message
                .as_ref()
                .and_then(|message| message.reply_to())
                .and_then(|addresses| addresses.first())
                .and_then(|mailbox| mailbox.address())
                .map(str::to_owned);
            self.message_id = message
                .as_ref()
                .and_then(|message| message.message_id())
                .map(str::to_owned);
            if let Some(addresses) = message.as_ref().and_then(|message| message.to()) {
                self.recipients = Contact::from_list(addresses);
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

    pub fn reply_address<'a>(&'a self, mail: &'a Mail) -> &'a str {
        self.reply_address.as_deref().unwrap_or(&mail.address)
    }

    pub fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    pub fn forward_text(&self) -> &str {
        &self.forward_text
    }

    pub fn set_action_error(&mut self, error: String) {
        self.action_error = Some(error);
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
        config: &'a Config,
        photos: Option<&'a crate::features::avatars::Avatars>,
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
            widget::responsive(move |size| {
                self.header(
                    mail,
                    folder,
                    config,
                    photos.and_then(|photos| photos.get(&mail.address)),
                    size.width,
                )
            })
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
        config: &Config,
        photo: Option<&cosmic::iced::widget::image::Handle>,
        width: f32,
    ) -> Element<'a, Message> {
        let style = config.message_actions;
        let recipients = mail.recipients.as_deref().unwrap_or(&self.recipients);
        let mut sender = widget::column::with_capacity(2)
            .push(widget::text::heading(mail.sender.as_str()))
            .width(Length::Fill)
            .spacing(2);
        if !mail.address.is_empty() && mail.address != mail.sender {
            sender = sender.push(address_menu(&mail.address, &mail.address));
        } else if !mail.address.is_empty() {
            sender = widget::column::with_capacity(1)
                .width(Length::Fill)
                .push(address_menu(&mail.address, &mail.address));
        }

        let sender: Element<_> = if config.show_recipient_pictures {
            widget::row::with_capacity(2)
                .width(Length::Fill)
                .spacing(8)
                .align_y(cosmic::iced::Alignment::Center)
                .push(mail.sender_picture(36.0, photo))
                .push(sender)
                .into()
        } else {
            sender.into()
        };

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
                (!self.reply_address(mail).is_empty()).then_some(Message::Reply),
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
            self.more_menu(mail, style),
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

        let mut recipient_row = widget::row::with_capacity(recipients.len() + 1).spacing(4);
        if !recipients.is_empty() {
            recipient_row = recipient_row.push(widget::text::caption(fl!("recipient")));
            for recipient in recipients {
                let label = match (recipient.name.as_str(), recipient.address.as_str()) {
                    ("", address) => address.to_owned(),
                    (name, "") => name.to_owned(),
                    (name, address) if name == address => address.to_owned(),
                    (name, address) => format!("{name} <{address}>"),
                };
                recipient_row = recipient_row.push(if recipient.address.is_empty() {
                    widget::text::caption(label).into()
                } else {
                    address_menu(label, &recipient.address)
                });
            }
        }
        let metadata = widget::row::with_capacity(2)
            .spacing(8)
            .push(widget::container(recipient_row.wrap()).width(Length::Fill))
            .push(widget::text::caption(mail.display_time()));

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
        if self.changing {
            header = header.push(widget::text::caption(fl!("mail-change-working")));
        } else if let Some(error) = &self.action_error {
            header = header.push(widget::text::caption(error.as_str()));
        }
        header.into()
    }
    fn more_menu(&self, mail: &Mail, style: MessageActions) -> Element<'static, Message> {
        let label = if mail.unread {
            fl!("mark-message-read")
        } else {
            fl!("mark-message-unread")
        };
        let mut items = vec![(
            label,
            (!self.changing).then_some(Message::Change(remote::Change::Seen(mail.unread))),
        )];
        if matches!(self.body, Body::Ready)
            && self
                .email
                .as_ref()
                .is_some_and(|view| !view.remote_images_enabled())
        {
            items.push((
                fl!("mail-show-remote-images"),
                Some(Message::ShowRemoteImages),
            ));
        }
        let root = action_button(
            style,
            "view-more-symbolic",
            fl!("more-message-actions"),
            Some(Message::Wake),
        );
        dropdown_menu(root, items)
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

fn address_menu(label: impl Into<String>, address: &str) -> Element<'static, Message> {
    let label =
        cosmic::iced::widget::rich_text::<(), _, _, _>([
            cosmic::iced::widget::span(label.into()).underline(true)
        ])
        .class(cosmic::theme::Text::Accent);
    let root = Element::from(
        widget::button::custom(label)
            .class(cosmic::theme::Button::MenuRoot)
            .padding([2, 4])
            .force_enabled(true),
    );
    dropdown_menu(
        root,
        vec![
            (
                fl!("copy-email-address"),
                Some(Message::CopyAddress(address.to_owned())),
            ),
            (
                fl!("compose-to-address"),
                Some(Message::ComposeTo(address.to_owned())),
            ),
        ],
    )
}

fn dropdown_menu(
    root: Element<'static, Message>,
    items: Vec<(String, Option<Message>)>,
) -> Element<'static, Message> {
    let children = items
        .into_iter()
        .map(|(label, message)| {
            menu::Tree::new(Element::from(
                menu::menu_button(vec![widget::text(label).into()]).on_press_maybe(message),
            ))
        })
        .collect();
    menu::bar(vec![menu::Tree::with_children(root, children)])
        .item_width(menu::ItemWidth::Uniform(220))
        .into()
}
