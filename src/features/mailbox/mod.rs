// SPDX-License-Identifier: GPL-3.0-only

mod cache;
mod list;
mod model;
pub mod remote;

pub use list::Message;
pub use model::{Folder, Mail, MailId};

use cosmic::iced::{
    futures::{StreamExt, stream},
    widget::scrollable,
};
use cosmic::widget::{self, nav_bar};

const VISIBLE_MESSAGE_LIMIT: usize = 500;

pub struct Mailbox {
    nav: nav_bar::Model,
    folder: Folder,
    sort_order: SortOrder,
    selected: Option<usize>,
    list_id: widget::Id,
    // Iced reconciles these buttons by position when filtering or sorting moves messages.
    row_ids: [widget::Id; VISIBLE_MESSAGE_LIMIT],
    list_offset: f32,
    request: u64,
    syncing: bool,
    downloads: Downloads,
    status: Status,
    contents: remote::FolderContents,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    NewestFirst,
    OldestFirst,
}

impl SortOrder {
    const ALL: [Self; 2] = [Self::NewestFirst, Self::OldestFirst];

    fn label(self) -> String {
        match self {
            Self::NewestFirst => crate::fl!("mail-sort-newest-first"),
            Self::OldestFirst => crate::fl!("mail-sort-oldest-first"),
        }
    }
}

#[derive(Default)]
struct Downloads {
    active: usize,
    saved: usize,
    total: usize,
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
            sort_order: SortOrder::default(),
            selected: None,
            list_id: widget::Id::unique(),
            row_ids: std::array::from_fn(|_| widget::Id::unique()),
            list_offset: 0.0,
            request: 0,
            syncing: false,
            downloads: Downloads::default(),
            status: Status::NoAccounts,
            contents: remote::FolderContents::default(),
        }
    }

    pub fn reload(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<Message> {
        self.selected = None;
        self.contents = remote::FolderContents::default();
        self.start_sync(accounts, true)
    }

    pub fn refresh_after_change(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<Message> {
        self.start_sync(accounts, false)
    }

    fn start_sync(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
        show_cached: bool,
    ) -> cosmic::Task<Message> {
        self.request += 1;
        self.syncing = false;
        self.downloads = Downloads::default();
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
        if show_cached {
            self.status = Status::Loading;
        }
        self.syncing = true;
        let request = self.request;
        let (send, receive) = tokio::sync::mpsc::unbounded_channel();
        let progress = stream::unfold(receive, |mut receive| async move {
            receive.recv().await.map(|event| (Some(event), receive))
        })
        .chain(stream::once(async { None }));
        let cached = if show_cached {
            cosmic::Task::perform(
                remote::cached(saved.clone(), self.folder),
                move |contents| Message::Cached(request, contents),
            )
        } else {
            cosmic::Task::none()
        };
        let sync = cosmic::Task::batch([
            cosmic::Task::perform(remote::load(saved, self.folder, send), move |contents| {
                Message::Loaded(request, contents)
            }),
            cosmic::Task::run(progress, move |event| Message::Download(request, event)),
        ]);
        cached.chain(sync)
    }

    pub fn update_download(&mut self, request: u64, event: Option<remote::DownloadEvent>) {
        if request != self.request {
            return;
        }
        match event {
            Some(remote::DownloadEvent::Started(total)) => {
                self.downloads.active += 1;
                self.downloads.total += total;
            }
            Some(remote::DownloadEvent::Saved(saved)) => self.downloads.saved += saved,
            Some(remote::DownloadEvent::Finished) => self.downloads.active -= 1,
            None => self.downloads = Downloads::default(),
        }
    }

    pub fn finish_load(
        &mut self,
        request: u64,
        contents: Option<remote::FolderContents>,
        synced: bool,
    ) -> Option<bool> {
        if request != self.request {
            return None;
        }
        self.syncing = !synced;
        let contents = contents?;
        let selection = self.selected().map(|mail| &mail.id);
        let selected =
            selection.and_then(|id| contents.messages.iter().position(|mail| &mail.id == id));
        let selection_removed = selection.is_some() && selected.is_none();
        self.selected = selected;
        self.contents = contents;
        self.status = Status::Loaded;
        Some(selection_removed)
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

    pub fn folder(&self) -> Folder {
        self.folder
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
        let mut visible = self.visible_messages(query);
        let first = visible.next()?.0;
        self.selected
            .filter(|&id| id == first || visible.any(|(visible_id, _)| visible_id == id))
            .or(Some(first))
    }

    pub fn remember_scroll(&mut self, offset: f32) {
        self.list_offset = offset;
    }

    pub fn restore_list_position(&mut self, query: &str) -> cosmic::Task<Message> {
        let scroll = self.scroll_to(self.list_offset);
        let focus = self
            .visible_messages(query)
            .position(|(index, _)| Some(index) == self.selected)
            .map_or_else(cosmic::Task::none, |position| {
                widget::button::focus(self.row_ids[position].clone())
            });
        cosmic::Task::batch([scroll, focus])
    }

    fn visible_messages<'a>(&'a self, query: &'a str) -> impl Iterator<Item = (usize, &'a Mail)> {
        self.matching_messages(query).take(VISIBLE_MESSAGE_LIMIT)
    }

    pub fn set_sort_order(&mut self, order: SortOrder) -> cosmic::Task<Message> {
        if self.sort_order == order {
            return cosmic::Task::none();
        }
        self.sort_order = order;
        self.scroll_to(0.0)
    }

    fn matching_messages<'a>(&'a self, query: &'a str) -> impl Iterator<Item = (usize, &'a Mail)> {
        let messages = &self.contents.messages;
        (0..messages.len())
            .map(move |position| {
                let index = match self.sort_order {
                    SortOrder::NewestFirst => position,
                    SortOrder::OldestFirst => messages.len() - 1 - position,
                };
                (index, &messages[index])
            })
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

    pub fn scroll_to(&mut self, offset: f32) -> cosmic::Task<Message> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::advanced::widget::{Tree, tree};

    #[test]
    fn filtering_and_sorting_preserve_message_button_state() {
        let mut mailbox = Mailbox::new();
        mailbox.status = Status::Loaded;
        mailbox.contents.messages = ["Other", "Match one", "Match two"]
            .into_iter()
            .enumerate()
            .map(|(index, subject)| Mail {
                id: MailId {
                    account: uuid::Uuid::nil(),
                    folder: "INBOX".into(),
                    uid_validity: 1,
                    uid: index as u32 + 1,
                },
                account_email: String::new(),
                unread: false,
                flagged: false,
                sender: String::new(),
                address: String::new(),
                subject: subject.into(),
                time: String::new(),
                timestamp: 3 - index as i64,
            })
            .collect();
        let mut state = Tree::new(mailbox.view("", false).as_widget());

        for (query, order, expected) in [
            ("Match", SortOrder::NewestFirst, &[1, 2][..]),
            ("Match", SortOrder::OldestFirst, &[2, 1][..]),
            ("Nothing", SortOrder::OldestFirst, &[][..]),
            ("", SortOrder::OldestFirst, &[2, 1, 0][..]),
        ] {
            let _ = mailbox.set_sort_order(order);
            tree::NAMED.with(|named| *named.borrow_mut() = state.take_all_named());
            state.diff(mailbox.view(query, false).as_widget_mut());
            tree::NAMED.with(|named| named.borrow_mut().clear());

            assert!(
                mailbox
                    .visible_messages(query)
                    .map(|(index, _)| index)
                    .eq(expected.iter().copied())
            );
            for (position, id) in mailbox.row_ids.iter().take(3).enumerate() {
                if position < expected.len() {
                    let button = state.find(id).expect("message button");
                    assert!(matches!(button.state, tree::State::Some(_)));
                } else {
                    assert!(state.find(id).is_none());
                }
            }
        }
    }
}
