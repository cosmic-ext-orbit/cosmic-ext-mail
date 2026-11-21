// SPDX-License-Identifier: GPL-3.0-only

use super::{Folder, Mailbox, Status, message_row_id, remote};
use crate::fl;
use cosmic::iced::Length;
use cosmic::{Element, widget};

#[derive(Clone, Debug)]
pub enum ListMessage {
    Select(usize),
    Step(isize),
    OpenSelected,
    Scrolled(f32),
    ResumeDraft,
    Refresh,
    AddAccount,
    Loaded(u64, remote::FolderContents),
}

impl Mailbox {
    pub fn view<'a>(&'a self, query: &'a str, has_draft: bool) -> Element<'a, ListMessage> {
        let mut rows = widget::column::with_capacity(self.contents.messages.len()).spacing(4);
        if self.folder == Folder::Drafts && has_draft {
            rows = rows.push(
                widget::button::standard(fl!("resume-draft")).on_press(ListMessage::ResumeDraft),
            );
        }
        match self.status {
            Status::NoAccounts => {
                rows = rows.push(widget::text::body(fl!("mail-no-accounts"))).push(
                    widget::button::standard(fl!("account-add")).on_press(ListMessage::AddAccount),
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
                let mut visible = self.visible_messages(query).peekable();
                if visible.peek().is_none()
                    && self.contents.failures.is_empty()
                    && !(self.folder == Folder::Drafts && has_draft)
                {
                    let empty = if query.trim().is_empty() {
                        fl!("folder-empty")
                    } else {
                        fl!("no-results")
                    };
                    rows = rows.push(widget::text::body(empty));
                }
                for (id, mail) in visible {
                    let sender = if mail.unread {
                        format!("• {}", mail.sender)
                    } else {
                        mail.sender.to_owned()
                    };
                    let line = widget::row::with_capacity(2)
                        .push(widget::text::body(sender).width(Length::Fill))
                        .push(widget::text::caption(mail.time.as_str()));
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
                    rows = rows.push(
                        widget::button::custom(row)
                            .id(message_row_id(id))
                            .class(cosmic::theme::Button::ListItem([6.0; 4]))
                            .width(Length::Fill)
                            .padding(10)
                            .selected(self.selected == Some(id))
                            .on_press(ListMessage::Select(id)),
                    );
                }
                if self.contents.limited {
                    rows = rows.push(widget::text::caption(fl!(
                        "mail-list-limit",
                        count = remote::PAGE_SIZE
                    )));
                }
            }
        }
        let header = widget::row::with_capacity(2)
            .push(widget::text::title3(self.folder.label()).width(Length::Fill))
            .push(widget::button::text(fl!("mail-refresh")).on_press_maybe(
                matches!(self.status, Status::Loaded).then_some(ListMessage::Refresh),
            ))
            .align_y(cosmic::iced::Alignment::Center);
        let content = widget::column::with_capacity(2)
            .push(header)
            .push(
                widget::scrollable(rows)
                    .id(self.list_id.clone())
                    .height(Length::Fill)
                    .on_scroll(|viewport| ListMessage::Scrolled(viewport.absolute_offset().y)),
            )
            .spacing(10)
            .height(Length::Fill);

        let card = widget::container(content)
            .class(cosmic::theme::Container::Card)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill);

        widget::container(card)
            .padding(8)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
