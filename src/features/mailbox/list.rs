// SPDX-License-Identifier: GPL-3.0-only

use super::{CONTENT_GUTTER, Change, Folder, Mail, Mailbox, Message, SortOrder, Status};
use crate::fl;
use cosmic::iced::{
    Border, Color, Length, Rectangle, Size,
    advanced::{Layout, Widget, layout, renderer, widget::Tree},
    mouse, window,
};
use cosmic::widget::menu;
use cosmic::{Element, widget};

#[derive(Clone, Copy, Debug)]
pub enum ComposeAction {
    Reply,
    Forward,
}

#[derive(Clone, Copy, Debug)]
pub enum RowAction {
    Open,
    Compose(ComposeAction),
    CopySender,
    ConfirmPermanentDelete,
    ComposeToSender,
    Change(Change),
}

impl Mailbox {
    pub fn view<'a>(
        &'a self,
        query: &'a str,
        drafts: &[(uuid::Uuid, String)],
        draft_read_error: bool,
        show_sender_pictures: bool,
        photos: Option<&crate::features::avatars::Avatars>,
        window_id: Option<window::Id>,
    ) -> Element<'a, Message> {
        let thread_rows = self.thread_rows(query);
        let card_radius = cosmic::theme::active().cosmic().corner_radii.radius_s;
        let page = self.page_for(drafts.len(), thread_rows.len());
        let mut rows = widget::column::with_capacity(self.contents.failures.len() + 3).spacing(4);
        if self.folder == Folder::Drafts {
            for (id, subject) in &drafts[page.drafts.clone()] {
                rows = rows.push(
                    widget::button::standard(subject.clone())
                        .leading_icon(widget::icon::from_name("document-edit-symbolic"))
                        .on_press(Message::ResumeDraft(*id)),
                );
            }
            if draft_read_error {
                rows = rows.push(widget::text::caption(fl!("draft-storage-error")));
            }
        }
        match self.status {
            Status::NoAccounts => {
                rows = rows.push(widget::text::body(fl!("mail-no-accounts"))).push(
                    widget::button::standard(fl!("account-add")).on_press(Message::AddAccount),
                );
            }
            Status::Loading => rows = rows.push(widget::text::body(fl!("mail-loading"))),
            Status::SettingsError(error) => rows = rows.push(widget::text::body(error.text())),
            Status::Loaded => {
                for (email, error) in &self.contents.failures {
                    rows = rows.push(widget::text::body(fl!(
                        "mail-account-failed",
                        email = email.as_str(),
                        error = error.text()
                    )));
                }
                if page.messages.is_empty()
                    && self.contents.failures.is_empty()
                    && page.drafts.is_empty()
                    && !self.syncing
                {
                    let empty = if query.trim().is_empty() {
                        fl!("folder-empty")
                    } else {
                        fl!("no-results")
                    };
                    rows = rows.push(widget::text::body(empty));
                }
                let mut messages = widget::column::with_capacity(page.messages.len()).spacing(4);
                for (position, entry) in thread_rows
                    .into_iter()
                    .skip(page.messages.start)
                    .take(page.messages.len())
                    .enumerate()
                {
                    let mail = entry.mail;
                    let unread = entry
                        .summary
                        .as_ref()
                        .map_or(mail.unread, |thread| thread.unread);
                    let flagged = entry
                        .summary
                        .as_ref()
                        .map_or(mail.flagged, |thread| thread.flagged);
                    let sender = if unread {
                        format!("• {}", mail.sender)
                    } else {
                        mail.sender.to_owned()
                    };
                    let line =
                        widget::row::with_capacity(3)
                            .push(widget::text::body(sender).width(Length::Fill))
                            .push_maybe(flagged.then(|| {
                                widget::icon::from_name("starred-symbolic").size(16).icon()
                            }))
                            .push(widget::text::caption(mail.display_time()))
                            .align_y(cosmic::iced::Alignment::Center)
                            .spacing(4);
                    let subject = if unread {
                        widget::text::body(mail.subject.as_str()).font(cosmic::font::bold())
                    } else {
                        widget::text::body(mail.subject.as_str())
                    };
                    let thread_button = entry.summary.as_ref().map(|thread| {
                        let tooltip = if thread.expanded {
                            fl!("mail-collapse-thread")
                        } else {
                            fl!("mail-expand-thread")
                        };
                        widget::button::text(fl!("mail-thread-count", count = thread.count))
                            .font_size(12)
                            .line_height(17)
                            .icon_size(14)
                            .leading_icon(widget::icon::from_name("chat-symbolic"))
                            .spacing(4)
                            .padding([2, 8])
                            .tooltip(tooltip)
                            .on_press(Message::ToggleThread(thread.key.clone()))
                    });
                    let row = widget::column::with_capacity(4)
                        .push(line)
                        .push(subject)
                        .push(widget::text::caption(mail.account_email.as_str()))
                        .push_maybe(thread_button)
                        .spacing(3);
                    let row: Element<_> = if show_sender_pictures {
                        widget::row::with_capacity(2)
                            .spacing(8)
                            .align_y(cosmic::iced::Alignment::Center)
                            .push(mail.sender_picture(
                                28.0,
                                photos.and_then(|photos| photos.get(&mail.address)),
                            ))
                            .push(row)
                            .into()
                    } else {
                        row.into()
                    };
                    let button = widget::button::custom(row)
                        .id(self.row_ids[position].clone())
                        .class(cosmic::theme::Button::ListItem(card_radius))
                        .width(Length::Fill)
                        .padding(10)
                        .selected(self.selected == Some(entry.index))
                        .on_press(Message::Select(mail.id.clone()));
                    let row: Element<_> = widget::container(self.row_menu(button, mail, window_id))
                        .width(Length::Fill)
                        .padding(1)
                        .style(|theme| {
                            use cosmic::iced::widget::container::Catalog as _;

                            let mut style = theme.style(&cosmic::theme::Container::Card);
                            style.border.color = theme.current_container().component.divider.into();
                            style.border.width = 1.0;
                            style
                        })
                        .into();
                    let row = if entry.parent.is_some() {
                        let reply = widget::row::with_capacity(2)
                            .push(widget::space::horizontal().width(24))
                            .push(row);
                        cosmic::iced::widget::Stack::new()
                            .push(reply)
                            .push(Element::new(ThreadConnector {
                                last: entry.last_reply,
                            }))
                            .into()
                    } else {
                        row
                    };
                    messages = messages.push(row);
                }
                rows = rows.push(messages);
            }
        }
        let sort_index = SortOrder::ALL
            .iter()
            .position(|&order| order == self.sort_order);
        let header = widget::row::with_capacity(3)
            .spacing(8)
            .push(widget::text::title3(self.folder.label()).width(Length::Fill))
            .push(widget::dropdown(
                SortOrder::ALL.map(SortOrder::label).to_vec(),
                sort_index,
                |index| Message::Sort(SortOrder::ALL[index]),
            ))
            .push(
                widget::button::icon(widget::icon::from_name("view-refresh-symbolic"))
                    .tooltip(if self.syncing {
                        fl!("mail-refresh-syncing")
                    } else {
                        fl!("mail-refresh")
                    })
                    .on_press_maybe(
                        (!self.syncing && matches!(self.status, Status::Loaded))
                            .then_some(Message::Refresh),
                    ),
            )
            .align_y(cosmic::iced::Alignment::Center);
        let pagination = (page.count > 1).then(|| {
            widget::row::with_capacity(5)
                .push(widget::space::horizontal())
                .push(
                    widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                        .tooltip(fl!("mail-previous-page"))
                        .on_press_maybe((page.index > 0).then_some(Message::TurnPage(-1))),
                )
                .push(widget::text::caption(fl!(
                    "mail-page",
                    page = (page.index + 1),
                    total = page.count
                )))
                .push(
                    widget::button::icon(widget::icon::from_name("go-next-symbolic"))
                        .tooltip(fl!("mail-next-page"))
                        .on_press_maybe(
                            (page.index + 1 < page.count).then_some(Message::TurnPage(1)),
                        ),
                )
                .push(widget::space::horizontal())
                .align_y(cosmic::iced::Alignment::Center)
                .spacing(8)
        });
        let content = widget::column::with_capacity(3)
            .push(header)
            .push(
                widget::scrollable(rows)
                    .spacing(CONTENT_GUTTER)
                    .id(self.list_id.clone())
                    .height(Length::Fill)
                    .on_scroll(|viewport| Message::Scrolled(viewport.absolute_offset().y)),
            )
            .push_maybe(pagination)
            .spacing(10)
            .height(Length::Fill);

        let card = widget::container(content)
            .class(cosmic::theme::Container::Card)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill);

        widget::container(card)
            .padding([0, CONTENT_GUTTER, CONTENT_GUTTER, CONTENT_GUTTER])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn row_menu<'a>(
        &self,
        button: impl Into<Element<'a, Message>>,
        mail: &Mail,
        window_id: Option<window::Id>,
    ) -> Element<'a, Message> {
        use RowAction::{Change, Compose, ComposeToSender, CopySender, Open};

        let has_sender = !mail.address.is_empty();
        let item = |label: String, action: RowAction| {
            menu::Tree::new(Element::from(
                menu::menu_button(vec![widget::text(label).into()])
                    .on_press(Message::RowAction(mail.id.clone(), action)),
            ))
        };
        let divider = || menu::Tree::new(Element::from(widget::divider::horizontal::light()));
        let mut items = vec![
            item(fl!("mail-open-message"), Open),
            item(fl!("reply"), Compose(ComposeAction::Reply)),
            item(fl!("forward"), Compose(ComposeAction::Forward)),
        ];
        if has_sender {
            items.extend([
                item(fl!("copy-email-address"), CopySender),
                item(fl!("compose-to-address"), ComposeToSender),
            ]);
        }
        items.extend([
            divider(),
            item(
                if mail.unread {
                    fl!("mark-message-read")
                } else {
                    fl!("mark-message-unread")
                },
                Change(super::Change::Seen(mail.unread)),
            ),
            item(
                if mail.flagged {
                    fl!("unstar-message")
                } else {
                    fl!("star-message")
                },
                Change(super::Change::Flagged(!mail.flagged)),
            ),
            divider(),
        ]);
        if matches!(self.folder, Folder::Archive | Folder::Junk | Folder::Trash) {
            items.push(item(
                fl!("mail-move-to-inbox"),
                Change(super::Change::Move(Folder::Inbox)),
            ));
        }
        for (folder, label) in [
            (Folder::Archive, fl!("archive-message")),
            (Folder::Junk, fl!("spam-message")),
            (Folder::Trash, fl!("delete-message")),
        ] {
            if folder != self.folder {
                items.push(item(label, Change(super::Change::Move(folder))));
            }
        }

        if self.folder == Folder::Trash {
            items.push(item(
                fl!("mail-delete-permanently"),
                RowAction::ConfirmPermanentDelete,
            ));
        }

        widget::context_menu(button, Some(items))
            .window_id(window_id.unwrap_or(window::Id::NONE))
            .on_surface_action(Message::MenuSurface)
            .into()
    }
}

struct ThreadConnector {
    last: bool,
}

impl Widget<Message, cosmic::Theme, cosmic::Renderer> for ThreadConnector {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(24.0), Length::Fill)
    }

    fn layout(
        &mut self,
        _: &mut Tree,
        _: &cosmic::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fixed(24.0), Length::Fill, Size::ZERO))
    }

    fn draw(
        &self,
        _: &Tree,
        renderer: &mut cosmic::Renderer,
        theme: &cosmic::Theme,
        _: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use cosmic::iced::advanced::Renderer as _;

        let bounds = layout.bounds();
        let center = bounds.center_x();
        let marker_y = bounds.center_y();
        let end = if self.last {
            marker_y
        } else {
            bounds.y + bounds.height + 2.0
        };
        let color: Color = theme.cosmic().accent.base.into();
        let color = Color { a: 0.65, ..color };
        let mut y = bounds.y - 2.0;
        while y + 2.0 <= end {
            let dot = Rectangle::new([center - 1.0, y].into(), Size::new(2.0, 2.0));
            if (dot.center_y() - marker_y).abs() >= 4.0 && viewport.intersects(&dot) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: dot,
                        border: Border::default().rounded(1.0),
                        ..Default::default()
                    },
                    color,
                );
            }
            y += 5.0;
        }
        let marker = Rectangle::new([center - 2.5, marker_y - 2.5].into(), Size::new(5.0, 5.0));
        if viewport.intersects(&marker) {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: marker,
                    border: Border {
                        color,
                        width: 1.0,
                        radius: 2.5.into(),
                    },
                    ..Default::default()
                },
                Color::TRANSPARENT,
            );
        }
    }
}
