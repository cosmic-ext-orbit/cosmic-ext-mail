// SPDX-License-Identifier: GPL-3.0-only

use super::{Body, Message, Reader};
use crate::{
    config::{Config, MessageActions},
    features::mailbox::{Change, Folder, Mail},
    fl,
};
use cosmic::{Element, iced::Length, widget, widget::menu};

const INLINE_ACTIONS_MIN_WIDTH: f32 = 520.0;

impl Reader {
    pub fn view<'a>(
        &'a self,
        mail: Option<&'a Mail>,
        folder: Folder,
        narrow: bool,
        config: &'a Config,
        photos: Option<&'a crate::features::avatars::Avatars>,
    ) -> Element<'a, Message> {
        let mut page = widget::column::with_capacity(4).spacing(8).padding(12);
        if narrow {
            page =
                page.push(widget::button::text(fl!("back-to-list")).on_press(Message::BackToList));
        }

        let Some(mail) = mail else {
            return page.push(widget::text::body(fl!("select-message"))).into();
        };
        let header = page.push(
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
        );
        let body: Element<_> = match &self.body {
            Body::Ready => self
                .email
                .as_ref()
                .expect("ready email view")
                .view_themed(self.theme_overrides.get(&mail.id).copied()),
            Body::Failed(error) => widget::column::with_capacity(2)
                .padding(12)
                .spacing(8)
                .push(widget::text(fl!("preview-failed", error = error.as_str())))
                .push(widget::button::text(fl!("mail-retry")).on_press(Message::Retry))
                .into(),
            Body::Empty | Body::Loading => {
                widget::container(widget::text::body(fl!("mail-body-loading")))
                    .padding(12)
                    .into()
            }
        };
        widget::container(
            widget::column::with_capacity(2)
                .push(header)
                .push(body)
                .spacing(crate::features::mailbox::CONTENT_GUTTER)
                .height(Length::Fill),
        )
        .class(cosmic::theme::Container::Card)
        .width(Length::Fill)
        .height(Length::Fill)
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
            sender = sender.push(address_menu(&mail.address, &mail.address, false));
        } else if !mail.address.is_empty() {
            sender = widget::column::with_capacity(1)
                .width(Length::Fill)
                .push(address_menu(&mail.address, &mail.address, false));
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

        let move_to = |destination| {
            (folder != destination).then_some(Message::Change(Change::Move(destination)))
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
                if folder == Folder::Trash {
                    fl!("mail-delete-permanently")
                } else {
                    fl!("delete-message")
                },
                if folder == Folder::Trash {
                    Some(Message::ConfirmPermanentDelete)
                } else {
                    move_to(Folder::Trash)
                },
            ),
            self.more_menu(mail, style),
            action_button(
                style,
                if mail.flagged {
                    "starred-symbolic"
                } else {
                    "non-starred-symbolic"
                },
                if mail.flagged {
                    fl!("unstar-message")
                } else {
                    fl!("star-message")
                },
                Some(Message::Change(Change::Flagged(!mail.flagged))),
            ),
        ])
        .spacing(4)
        .wrap();

        let mut recipient_row = widget::row::with_capacity(recipients.len() + 1)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(4);
        if !recipients.is_empty() {
            recipient_row = recipient_row.push(widget::text::body(fl!("recipient")));
            for recipient in recipients {
                let label = match (recipient.name.as_str(), recipient.address.as_str()) {
                    ("", address) => address.to_owned(),
                    (name, "") => name.to_owned(),
                    (name, address) if name == address => address.to_owned(),
                    (name, address) => format!("{name} <{address}>"),
                };
                recipient_row = recipient_row.push(if recipient.address.is_empty() {
                    widget::text::body(label).into()
                } else {
                    address_menu(label, &recipient.address, true)
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
        if !self.attachments.is_empty() {
            let mut files = widget::row::with_capacity(self.attachments.len()).spacing(6);
            for (index, file) in self.attachments.iter().enumerate() {
                let size = fl!(
                    "attachment-size",
                    size = format!("{:.1}", file.size as f64 / 1024.0)
                );
                files = files.push(
                    widget::button::standard(fl!(
                        "mail-attachment-label",
                        name = file.name.as_str(),
                        size = size.as_str()
                    ))
                    .leading_icon(widget::icon::from_name("mail-attachment-symbolic"))
                    .on_press(Message::SaveAttachment(self.request, index)),
                );
            }
            header = header
                .push(widget::text::caption(fl!("mail-attachments")))
                .push(
                    widget::container(widget::scrollable(files.wrap()).height(Length::Shrink))
                        .max_height(112),
                );
        }
        header.into()
    }
    fn more_menu(&self, mail: &Mail, style: MessageActions) -> Element<'static, Message> {
        let label = if mail.unread {
            fl!("mark-message-read")
        } else {
            fl!("mark-message-unread")
        };
        let mut items = vec![(label, Some(Message::Change(Change::Seen(mail.unread))))];
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
        if matches!(self.body, Body::Ready) {
            let theme_override = self.theme_overrides.get(&mail.id).copied();
            let is_dark = theme_override.unwrap_or_else(cosmic::theme::is_dark);
            items.push((
                if is_dark {
                    fl!("mail-use-light-theme")
                } else {
                    fl!("mail-use-dark-theme")
                },
                Some(Message::SetTheme(mail.id.clone(), Some(!is_dark))),
            ));
            if theme_override.is_some() {
                items.push((
                    fl!("mail-use-desktop-theme"),
                    Some(Message::SetTheme(mail.id.clone(), None)),
                ));
            }
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

fn address_menu(
    label: impl Into<String>,
    address: &str,
    recipient: bool,
) -> Element<'static, Message> {
    let label =
        cosmic::iced::widget::rich_text::<(), _, _, _>([
            cosmic::iced::widget::span(label.into()).underline(true)
        ])
        .class(cosmic::theme::Text::Accent);
    let label = if recipient {
        label
            .size(14.0)
            .line_height(cosmic::iced::widget::text::LineHeight::Absolute(
                21.0.into(),
            ))
    } else {
        label
    };
    let root = Element::from(
        widget::button::custom(label)
            .class(cosmic::theme::Button::Text)
            .padding([3, 6])
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
