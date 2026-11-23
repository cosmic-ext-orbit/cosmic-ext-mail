// SPDX-License-Identifier: GPL-3.0-only

use super::{CONTENT_GUTTER, Folder, Mail, MailId, Mailbox, SortOrder, Status, remote};
use crate::fl;
use cosmic::iced::{Length, window};
use cosmic::widget::menu;
use cosmic::{Element, widget};

#[derive(Clone, Debug)]
pub enum Message {
    Select(usize),
    RowAction(MailId, RowAction),
    MenuSurface(cosmic::surface::Action<Message>),
    Step(isize),
    TurnPage(isize),
    OpenSelected,
    Scrolled(f32),
    ResumeDraft(uuid::Uuid),
    Refresh,
    Sort(SortOrder),
    AddAccount,
    Cached(u64, Option<remote::FolderContents>),
    Loaded(u64, remote::FolderContents),
    Download(u64, Option<remote::DownloadEvent>),
}

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
    ComposeToSender,
    Change(remote::Change),
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
        let page = self.page(query, drafts.len());
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
                {
                    let empty = if query.trim().is_empty() {
                        fl!("folder-empty")
                    } else {
                        fl!("no-results")
                    };
                    rows = rows.push(widget::text::body(empty));
                }
                let mut messages = widget::column::with_capacity(page.messages.len()).spacing(4);
                for (position, (id, mail)) in self
                    .matching_messages(query)
                    .skip(page.messages.start)
                    .take(page.messages.len())
                    .enumerate()
                {
                    let sender = if mail.unread {
                        format!("• {}", mail.sender)
                    } else {
                        mail.sender.to_owned()
                    };
                    let line = widget::row::with_capacity(3)
                        .push(widget::text::body(sender).width(Length::Fill))
                        .push_maybe(mail.flagged.then(|| {
                            widget::icon::from_name("mail-mark-important-symbolic")
                                .size(16)
                                .icon()
                        }))
                        .push(widget::text::caption(mail.display_time()))
                        .align_y(cosmic::iced::Alignment::Center)
                        .spacing(4);
                    let subject = if mail.unread {
                        widget::text::body(mail.subject.as_str()).font(cosmic::font::bold())
                    } else {
                        widget::text::body(mail.subject.as_str())
                    };
                    let row = widget::column::with_capacity(3)
                        .push(line)
                        .push(subject)
                        .push(widget::text::caption(mail.account_email.as_str()))
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
                        .class(cosmic::theme::Button::ListItem([6.0; 4]))
                        .width(Length::Fill)
                        .padding(10)
                        .selected(self.selected == Some(id))
                        .on_press(Message::Select(id));
                    messages = messages.push(self.row_menu(button, mail, window_id));
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
            .padding(CONTENT_GUTTER)
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
                Change(remote::Change::Seen(mail.unread)),
            ),
            item(
                if mail.flagged {
                    fl!("unstar-message")
                } else {
                    fl!("star-message")
                },
                Change(remote::Change::Flagged(!mail.flagged)),
            ),
            divider(),
        ]);
        if matches!(self.folder, Folder::Archive | Folder::Junk | Folder::Trash) {
            items.push(item(
                fl!("mail-move-to-inbox"),
                Change(remote::Change::Move(Folder::Inbox)),
            ));
        }
        for (folder, label) in [
            (Folder::Archive, fl!("archive-message")),
            (Folder::Junk, fl!("spam-message")),
            (Folder::Trash, fl!("delete-message")),
        ] {
            if folder != self.folder {
                items.push(item(label, Change(remote::Change::Move(folder))));
            }
        }

        widget::context_menu(button, Some(items))
            .window_id(window_id.unwrap_or(window::Id::NONE))
            .on_surface_action(Message::MenuSurface)
            .into()
    }
}
