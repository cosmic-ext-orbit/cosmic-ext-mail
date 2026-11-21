// SPDX-License-Identifier: GPL-3.0-only

mod list;
mod model;
pub mod remote;

pub use list::ListMessage;
pub use model::{Folder, Mail, MailId};

use cosmic::iced::widget::scrollable;
use cosmic::widget::{self, nav_bar};

pub struct Mailbox {
    nav: nav_bar::Model,
    folder: Folder,
    selected: Option<usize>,
    list_id: widget::Id,
    list_offset: f32,
    request: u64,
    status: Status,
    contents: remote::FolderContents,
}

enum Status {
    NoAccounts,
    Loading,
    Loaded,
    SettingsError(crate::features::accounts::Error),
}

impl Mailbox {
    pub fn new() -> Self {
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

        Self {
            nav,
            folder: Folder::Inbox,
            selected: None,
            list_id: widget::Id::unique(),
            list_offset: 0.0,
            request: 0,
            status: Status::NoAccounts,
            contents: remote::FolderContents::default(),
        }
    }

    pub fn reload(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<ListMessage> {
        self.request += 1;
        self.selected = None;
        self.contents = remote::FolderContents::default();
        let saved = match accounts.saved() {
            Ok([]) => {
                self.status = Status::NoAccounts;
                return cosmic::Task::none();
            }
            Ok(accounts) => accounts.to_vec(),
            Err(error) => {
                self.status = Status::SettingsError(error);
                return cosmic::Task::none();
            }
        };
        self.status = Status::Loading;
        let request = self.request;
        cosmic::Task::perform(remote::load(saved, self.folder), move |contents| {
            ListMessage::Loaded(request, contents)
        })
    }

    pub fn finish_load(&mut self, request: u64, contents: remote::FolderContents) -> bool {
        if request != self.request {
            return false;
        }
        self.contents = contents;
        self.status = Status::Loaded;
        true
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
        self.selected.and_then(|id| self.contents.messages.get(id))
    }

    pub fn set_selection(&mut self, selection: Option<usize>) -> bool {
        if self.selected == selection
            || selection.is_some_and(|id| id >= self.contents.messages.len())
        {
            return false;
        }
        self.selected = selection;
        true
    }

    pub fn selection_for(&self, query: &str) -> Option<usize> {
        self.selected
            .filter(|&id| self.contents.messages[id].matches(query))
            .or_else(|| self.visible_messages(query).next().map(|(id, _)| id))
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
        self.contents
            .messages
            .iter()
            .enumerate()
            .filter(move |(_, mail)| mail.matches(query))
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
