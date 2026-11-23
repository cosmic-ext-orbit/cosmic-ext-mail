// SPDX-License-Identifier: GPL-3.0-only

mod cache;
mod list;
mod model;
mod notifications;
pub mod remote;

pub use list::Message;
pub use model::{Contact, Folder, Mail, MailId};

use cosmic::iced::{
    futures::{StreamExt, stream},
    widget::scrollable,
};
use cosmic::widget::{self, nav_bar};

const VISIBLE_MESSAGE_LIMIT: usize = 500;
pub const CONTENT_GUTTER: u16 = 8;

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
    sync_task: Option<cosmic::iced::task::Handle>,
    downloads: Downloads,
    status: Status,
    contents: remote::FolderContents,
    pending_changes: Vec<PendingChange>,
}

struct PendingChange {
    folder: Folder,
    original: Mail,
    change: remote::Change,
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

pub enum FetchActivity {
    Checking,
    Downloading { saved: usize, total: usize },
}

pub enum Activity {
    Changing,
    NoAccounts,
    SettingsError,
    SyncFailed,
    Idle,
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
            sync_task: None,
            downloads: Downloads::default(),
            status: Status::NoAccounts,
            contents: remote::FolderContents::default(),
            pending_changes: Vec::new(),
        }
    }

    pub fn fetch_activity(&self) -> Option<FetchActivity> {
        if self.downloads.active > 0 && self.downloads.total > 0 {
            return Some(FetchActivity::Downloading {
                saved: self.downloads.saved,
                total: self.downloads.total,
            });
        }
        self.syncing.then_some(FetchActivity::Checking)
    }

    pub fn activity(&self) -> Activity {
        if !self.pending_changes.is_empty() {
            return Activity::Changing;
        }
        match self.status {
            Status::NoAccounts => Activity::NoAccounts,
            Status::SettingsError(_) => Activity::SettingsError,
            Status::Loaded if !self.contents.failures.is_empty() => Activity::SyncFailed,
            Status::Loading | Status::Loaded => Activity::Idle,
        }
    }

    pub fn reload(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<Message> {
        self.selected = None;
        self.contents = remote::FolderContents::default();
        self.start_sync(accounts, true, true)
    }

    pub fn refresh_after_change(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<Message> {
        self.start_sync(accounts, false, true)
    }

    pub fn refresh(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<Message> {
        if self.syncing {
            return cosmic::Task::none();
        }
        self.start_sync(accounts, false, true)
    }

    pub fn poll_inbox(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
    ) -> cosmic::Task<Message> {
        if self.syncing {
            return cosmic::Task::none();
        }
        self.start_sync(accounts, false, false)
    }

    pub fn is_current_request(&self, request: u64) -> bool {
        request == self.request
    }

    pub fn cancel_sync(&mut self) {
        if let Some(task) = self.sync_task.take() {
            task.abort();
            self.request += 1;
            self.syncing = false;
            self.downloads = Downloads::default();
        }
    }

    fn start_sync(
        &mut self,
        accounts: &crate::features::accounts::Accounts,
        show_cached: bool,
        prefetch: bool,
    ) -> cosmic::Task<Message> {
        self.cancel_sync();
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
            cosmic::Task::perform(
                remote::load(saved, self.folder, send, prefetch),
                move |contents| Message::Loaded(request, contents),
            ),
            cosmic::Task::run(progress, move |event| Message::Download(request, event)),
        ]);
        let (task, handle) = cached.chain(sync).abortable();
        self.sync_task = Some(handle);
        task
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
        let mut contents = contents?;
        for pending in self
            .pending_changes
            .iter()
            .filter(|pending| pending.folder == self.folder)
        {
            pending
                .change
                .apply(&mut contents.messages, &pending.original.id);
        }
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

    pub fn selected_change_pending(&self) -> bool {
        self.selected().is_some_and(|mail| {
            self.pending_changes
                .iter()
                .any(|pending| pending.original.id == mail.id)
        })
    }

    pub fn begin_change(&mut self, change: remote::Change) -> Option<(Folder, MailId)> {
        if self.selected_change_pending() {
            return None;
        }
        let original = self.selected()?.clone();
        let id = original.id.clone();
        let folder = self.folder;
        change.apply(&mut self.contents.messages, &id);
        if matches!(change, remote::Change::Move(_)) {
            self.selected = None;
        }
        self.pending_changes.push(PendingChange {
            folder,
            original,
            change,
        });
        Some((folder, id))
    }

    // Returns whether a removed message was restored as the current selection.
    pub fn finish_change(&mut self, id: &MailId, succeeded: bool) -> bool {
        let Some(index) = self
            .pending_changes
            .iter()
            .position(|pending| pending.original.id == *id)
        else {
            return false;
        };
        let PendingChange {
            folder,
            original,
            change,
        } = self.pending_changes.remove(index);
        if succeeded || folder != self.folder {
            return false;
        }

        match change {
            remote::Change::Seen(_) => {
                remote::Change::Seen(!original.unread).apply(&mut self.contents.messages, id);
            }
            remote::Change::Flagged(_) => {
                remote::Change::Flagged(original.flagged).apply(&mut self.contents.messages, id);
            }
            remote::Change::Move(_) => {
                let existing_selection = self.selected().map(|mail| mail.id.clone());
                if !self.contents.messages.iter().any(|mail| mail.id == *id) {
                    self.contents.messages.push(original);
                    self.contents.sort();
                }
                let selected_id = existing_selection.as_ref().unwrap_or(id);
                self.selected = self
                    .contents
                    .messages
                    .iter()
                    .position(|mail| mail.id == *selected_id);
                return existing_selection.is_none() && self.selected.is_some();
            }
        }
        false
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
        if self.selected.is_none()
            && self.pending_changes.iter().any(|pending| {
                pending.folder == self.folder && matches!(pending.change, remote::Change::Move(_))
            })
        {
            return None;
        }
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

    pub fn visible_sender_addresses<'a>(&'a self, query: &'a str) -> impl Iterator<Item = &'a str> {
        self.visible_messages(query)
            .map(|(_, mail)| mail.address.as_str())
            .chain(self.selected().map(|mail| mail.address.as_str()))
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

    fn mail(uid: u32, subject: &str) -> Mail {
        Mail {
            id: MailId {
                account: uuid::Uuid::nil(),
                folder: "INBOX".into(),
                uid_validity: 1,
                uid,
            },
            account_email: String::new(),
            unread: false,
            flagged: false,
            sender: String::new(),
            address: String::new(),
            recipients: None,
            subject: subject.into(),
            time: String::new(),
            timestamp: 4 - i64::from(uid),
        }
    }

    #[test]
    fn optimistic_changes_survive_stale_sync_and_rollback_the_affected_message() {
        use remote::Change;

        let mut first = mail(1, "First");
        first.unread = true;
        let second = mail(2, "Second");
        let server = remote::FolderContents {
            messages: vec![first.clone(), second.clone()],
            ..Default::default()
        };
        let mut mailbox = Mailbox::new();
        mailbox.contents = server.clone();
        mailbox.set_selection(Some(0));

        mailbox.begin_change(Change::Seen(true)).unwrap();
        mailbox.syncing = true;
        mailbox.update_download(mailbox.request, Some(remote::DownloadEvent::Started(2)));
        assert!(matches!(mailbox.activity(), Activity::Changing));
        assert!(matches!(
            mailbox.fetch_activity(),
            Some(FetchActivity::Downloading { saved: 0, total: 2 })
        ));
        assert!(!mailbox.selected().unwrap().unread);
        mailbox.finish_load(mailbox.request, Some(server.clone()), true);
        assert!(!mailbox.selected().unwrap().unread);

        mailbox.set_selection(Some(1));
        mailbox.begin_change(Change::Flagged(true)).unwrap();
        mailbox.finish_change(&first.id, false);
        assert!(mailbox.contents.messages[0].unread);
        assert!(mailbox.selected().unwrap().flagged);
        mailbox.finish_change(&second.id, true);

        mailbox.set_selection(Some(0));
        mailbox.begin_change(Change::Move(Folder::Archive)).unwrap();
        assert_eq!(mailbox.contents.messages.len(), 1);
        mailbox.finish_load(mailbox.request, Some(server), true);
        assert_eq!(mailbox.selection_for(""), None);
        assert!(mailbox.finish_change(&first.id, false));
        assert_eq!(mailbox.selected().unwrap().id, first.id);
        assert_eq!(mailbox.contents.messages.len(), 2);
    }

    #[test]
    fn filtering_and_sorting_preserve_message_button_state() {
        let mut mailbox = Mailbox::new();
        mailbox.status = Status::Loaded;
        mailbox.contents.messages = ["Other", "Match one", "Match two"]
            .into_iter()
            .enumerate()
            .map(|(index, subject)| mail(index as u32 + 1, subject))
            .collect();
        let mut state = Tree::new(mailbox.view("", &[], false, false, None).as_widget());

        for (query, order, pictures, expected) in [
            ("Match", SortOrder::NewestFirst, false, &[1, 2][..]),
            ("Match", SortOrder::OldestFirst, true, &[2, 1][..]),
            ("Nothing", SortOrder::OldestFirst, true, &[][..]),
            ("", SortOrder::OldestFirst, false, &[2, 1, 0][..]),
        ] {
            let _ = mailbox.set_sort_order(order);
            tree::NAMED.with(|named| *named.borrow_mut() = state.take_all_named());
            state.diff(
                mailbox
                    .view(query, &[], false, pictures, None)
                    .as_widget_mut(),
            );
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
