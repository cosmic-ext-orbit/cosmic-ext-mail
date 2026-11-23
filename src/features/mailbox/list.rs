// SPDX-License-Identifier: GPL-3.0-only

use super::{CONTENT_GUTTER, Folder, Mailbox, SortOrder, Status, VISIBLE_MESSAGE_LIMIT, remote};
use crate::fl;
use cosmic::iced::Length;
use cosmic::{Element, widget};

#[derive(Clone, Debug)]
pub enum Message {
    Select(usize),
    Step(isize),
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

impl Mailbox {
    pub fn view<'a>(
        &'a self,
        query: &'a str,
        drafts: &[(uuid::Uuid, String)],
        draft_read_error: bool,
        show_sender_pictures: bool,
        photos: Option<&crate::features::avatars::Avatars>,
    ) -> Element<'a, Message> {
        let mut rows = widget::column::with_capacity(self.contents.failures.len() + 3).spacing(4);
        if self.folder == Folder::Drafts {
            for (id, subject) in drafts {
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
                let mut matches = self.matching_messages(query).peekable();
                if matches.peek().is_none()
                    && self.contents.failures.is_empty()
                    && !(self.folder == Folder::Drafts && !drafts.is_empty())
                {
                    let empty = if query.trim().is_empty() {
                        fl!("folder-empty")
                    } else {
                        fl!("no-results")
                    };
                    rows = rows.push(widget::text::body(empty));
                }
                let mut messages = widget::column::with_capacity(
                    self.contents.messages.len().min(VISIBLE_MESSAGE_LIMIT),
                )
                .spacing(4);
                for (position, (id, mail)) in
                    matches.by_ref().take(VISIBLE_MESSAGE_LIMIT).enumerate()
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
                    messages = messages.push(
                        widget::button::custom(row)
                            .id(self.row_ids[position].clone())
                            .class(cosmic::theme::Button::ListItem([6.0; 4]))
                            .width(Length::Fill)
                            .padding(10)
                            .selected(self.selected == Some(id))
                            .on_press(Message::Select(id)),
                    );
                }
                rows = rows.push(messages);
                if matches.peek().is_some() {
                    rows = rows.push(widget::text::caption(fl!(
                        "mail-visible-limit",
                        count = VISIBLE_MESSAGE_LIMIT
                    )));
                }
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
        let content = widget::column::with_capacity(2)
            .push(header)
            .push(
                widget::scrollable(rows)
                    .spacing(CONTENT_GUTTER)
                    .id(self.list_id.clone())
                    .height(Length::Fill)
                    .on_scroll(|viewport| Message::Scrolled(viewport.absolute_offset().y)),
            )
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
}
