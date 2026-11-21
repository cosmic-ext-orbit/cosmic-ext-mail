// SPDX-License-Identifier: GPL-3.0-only

use super::{Folder, Mailbox, message_row_id};
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
    UndoMove,
}

impl Mailbox {
    pub fn view<'a>(&'a self, query: &'a str, has_draft: bool) -> Element<'a, ListMessage> {
        let mut rows = widget::column::with_capacity(self.messages.len()).spacing(4);
        if self.folder == Folder::Drafts && has_draft {
            rows = rows.push(
                widget::button::standard(fl!("resume-draft")).on_press(ListMessage::ResumeDraft),
            );
        } else {
            let mut visible = self.visible_messages(query).peekable();
            if visible.peek().is_none() {
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
                    .push(widget::text::caption(mail.time));
                let subject = if mail.unread {
                    widget::text::body(mail.subject).font(cosmic::font::bold())
                } else {
                    widget::text::body(mail.subject)
                };
                let row = widget::column::with_capacity(3)
                    .push(line)
                    .push(subject)
                    .push(widget::text::caption(mail.snippet))
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
        }
        let content = widget::column::with_capacity(2)
            .push(widget::text::title3(self.folder.label()))
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

    pub fn undo_view(&self) -> Option<Element<'_, ListMessage>> {
        self.undo?;
        Some(
            widget::row::with_capacity(2)
                .push(widget::text(fl!("message-moved")))
                .push(widget::button::text(fl!("undo")).on_press(ListMessage::UndoMove))
                .spacing(8)
                .padding([0, 12])
                .into(),
        )
    }
}
