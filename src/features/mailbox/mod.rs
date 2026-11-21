// SPDX-License-Identifier: GPL-3.0-only

mod fixtures;
mod list;
mod model;

pub use list::ListMessage;
pub use model::{Folder, Mail};

use cosmic::iced::widget::scrollable;
use cosmic::widget::{self, nav_bar};

pub struct Mailbox {
    nav: nav_bar::Model,
    folder: Folder,
    messages: Vec<Mail>,
    selected: Option<usize>,
    list_id: widget::Id,
    list_offset: f32,
    undo: Option<(usize, Folder)>,
}

impl Mailbox {
    pub fn new() -> Self {
        let messages = fixtures::sample_mail();
        let selected = messages
            .iter()
            .position(|mail| mail.folder == Folder::Inbox);
        let mut nav = nav_bar::Model::default();
        for folder in Folder::ALL {
            let item = nav
                .insert()
                .text(folder.label())
                .icon(widget::icon::from_name(folder.icon()))
                .data(folder);
            if folder == Folder::Inbox {
                item.activate();
            }
        }

        let mut mailbox = Self {
            nav,
            folder: Folder::Inbox,
            messages,
            selected,
            list_id: widget::Id::unique(),
            list_offset: 0.0,
            undo: None,
        };
        mailbox.update_folder_counts();
        mailbox
    }

    pub fn nav_model(&self) -> &nav_bar::Model {
        &self.nav
    }

    pub fn select_folder(&mut self, id: nav_bar::Id) -> bool {
        let Some(&folder) = self.nav.data::<Folder>(id) else {
            return false;
        };
        if folder == self.folder {
            return false;
        }
        self.nav.activate(id);
        self.folder = folder;
        true
    }

    pub fn selected(&self) -> Option<&Mail> {
        self.selected.map(|id| &self.messages[id])
    }

    pub fn set_selection(&mut self, selection: Option<usize>) -> bool {
        if self.selected == selection {
            return false;
        }
        self.selected = selection;
        true
    }

    pub fn selection_for(&self, query: &str) -> Option<usize> {
        self.selected
            .filter(|&id| {
                self.messages[id].folder == self.folder && self.messages[id].matches(query)
            })
            .or_else(|| self.visible_messages(query).next().map(|(id, _)| id))
    }

    pub fn mark_selected_read(&mut self) {
        if let Some(id) = self.selected {
            self.messages[id].unread = false;
            self.update_folder_counts();
        }
    }

    pub fn toggle_selected_unread(&mut self) {
        if let Some(id) = self.selected {
            self.messages[id].unread = !self.messages[id].unread;
            self.update_folder_counts();
        }
    }

    pub fn move_selected(&mut self, folder: Folder, query: &str) -> Option<usize> {
        let id = self.selected?;
        let next = self
            .visible_messages(query)
            .find(|(candidate, _)| *candidate > id)
            .or_else(|| {
                self.visible_messages(query)
                    .rev()
                    .find(|(candidate, _)| *candidate < id)
            })
            .map(|(candidate, _)| candidate);
        let previous = std::mem::replace(&mut self.messages[id].folder, folder);
        self.undo = Some((id, previous));
        self.update_folder_counts();
        next
    }

    pub fn undo_move(&mut self) -> bool {
        let Some((id, folder)) = self.undo.take() else {
            return false;
        };
        self.messages[id].folder = folder;
        self.update_folder_counts();
        true
    }

    pub fn remember_scroll(&mut self, offset: f32) {
        self.list_offset = offset;
    }

    pub fn restore_list_position(&mut self) -> cosmic::Task<ListMessage> {
        let scroll = self.scroll_to(self.list_offset);
        let focus = self.selected.map_or_else(cosmic::Task::none, |id| {
            widget::button::focus(message_row_id(id))
        });
        cosmic::Task::batch([scroll, focus])
    }

    fn visible_messages<'a>(
        &'a self,
        query: &'a str,
    ) -> impl DoubleEndedIterator<Item = (usize, &'a Mail)> {
        self.messages
            .iter()
            .enumerate()
            .filter(move |(_, mail)| mail.folder == self.folder && mail.matches(query))
    }

    fn update_folder_counts(&mut self) {
        let ids: Vec<_> = self.nav.iter().collect();
        for id in ids {
            let Some(&folder) = self.nav.data::<Folder>(id) else {
                continue;
            };
            let unread = self
                .messages
                .iter()
                .filter(|mail| mail.folder == folder && mail.unread)
                .count();
            let label = folder.label();
            self.nav.text_set(
                id,
                if unread == 0 {
                    label
                } else {
                    format!("{label}  {unread}")
                },
            );
        }
    }

    pub fn step_selection(&self, query: &str, step: isize) -> Option<usize> {
        let ids: Vec<_> = self.visible_messages(query).map(|(id, _)| id).collect();
        let Some(position) = ids.iter().position(|&id| Some(id) == self.selected) else {
            return ids.first().copied();
        };
        let index = position.saturating_add_signed(step).min(ids.len() - 1);
        Some(ids[index])
    }

    pub fn scroll_to(&mut self, offset: f32) -> cosmic::Task<ListMessage> {
        self.list_offset = offset;
        scrollable::scroll_to(
            self.list_id.clone(),
            scrollable::AbsoluteOffset {
                x: None,
                y: Some(offset),
            },
        )
    }
}

fn message_row_id(id: usize) -> widget::Id {
    widget::Id::new(format!("message-row-{id}"))
}
