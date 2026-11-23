// SPDX-License-Identifier: GPL-3.0-only

mod cache;
mod list;
mod model;
mod notifications;
pub mod remote;

pub use list::{ComposeAction, Message, RowAction};
pub use model::{Contact, Folder, Mail, MailId};

use cosmic::iced::{
    futures::{StreamExt, stream},
    widget::scrollable,
};
use cosmic::widget::{self, nav_bar};

const MESSAGES_PER_PAGE: usize = 50;
pub const CONTENT_GUTTER: u16 = 8;

pub struct Mailbox {
    nav: nav_bar::Model,
    folder: Folder,
    sort_order: SortOrder,
    selected: Option<usize>,
    list_id: widget::Id,
    // Iced reconciles these buttons by position when filtering or sorting moves messages.
    row_ids: [widget::Id; MESSAGES_PER_PAGE],
    page: usize,
    list_offset: f32,
    request: u64,
    syncing: bool,
    sync_task: Option<cosmic::iced::task::Handle>,
    downloads: Downloads,
    status: Status,
    contents: remote::FolderContents,
    pending_changes: Vec<PendingChange>,
}

struct Page {
    index: usize,
    count: usize,
    drafts: std::ops::Range<usize>,
    messages: std::ops::Range<usize>,
}

struct PendingChange {
    folder: Folder,
    original: Mail,
    change: remote::Change,
    queued: Vec<remote::Change>,
    was_selected: bool,
}

impl PendingChange {
    fn apply(&self, messages: &mut Vec<Mail>) {
        // A sync result can predate a change confirmed earlier in this queue.
        if let Some(mail) = messages.iter_mut().find(|mail| mail.id == self.original.id) {
            mail.unread = self.original.unread;
            mail.flagged = self.original.flagged;
        }
        for change in std::iter::once(self.change).chain(self.queued.iter().copied()) {
            change.apply(messages, &self.original.id);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChangeStart {
    Send,
    Queued,
    Unavailable,
}

#[derive(Default)]
pub struct ChangeCompletion {
    pub restored_selection: bool,
    pub next: Option<remote::Change>,
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
            page: 0,
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
        self.page = 0;
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
            pending.apply(&mut contents.messages);
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

    pub fn folder_id(&self, folder: Folder) -> Option<nav_bar::Id> {
        self.nav
            .iter()
            .find(|&id| self.nav.data::<Folder>(id) == Some(&folder))
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

    pub fn message_index(&self, id: &MailId) -> Option<usize> {
        self.contents
            .messages
            .iter()
            .position(|mail| mail.id == *id)
    }

    pub fn message(&self, id: &MailId) -> Option<&Mail> {
        self.message_index(id)
            .and_then(|index| self.contents.messages.get(index))
    }

    pub fn begin_change(&mut self, id: &MailId, change: remote::Change) -> ChangeStart {
        let Some(index) = self.message_index(id) else {
            return ChangeStart::Unavailable;
        };
        let original = &self.contents.messages[index];
        if !change.affects(original) {
            return ChangeStart::Unavailable;
        }
        let previous = self
            .pending_changes
            .iter_mut()
            .find(|pending| pending.original.id == *id);
        let was_selected = self.selected == Some(index);
        let moving = matches!(change, remote::Change::Move(_));
        let start = if let Some(pending) = previous {
            pending
                .queued
                .retain(|queued| std::mem::discriminant(queued) != std::mem::discriminant(&change));
            pending.queued.push(change);
            if moving {
                pending.was_selected = was_selected;
            }
            ChangeStart::Queued
        } else {
            self.pending_changes.push(PendingChange {
                folder: self.folder,
                original: original.clone(),
                change,
                queued: Vec::new(),
                was_selected: moving && was_selected,
            });
            ChangeStart::Send
        };

        change.apply(&mut self.contents.messages, id);
        if moving {
            self.selected = self
                .selected
                .and_then(|selection| match selection.cmp(&index) {
                    std::cmp::Ordering::Less => Some(selection),
                    std::cmp::Ordering::Equal => None,
                    std::cmp::Ordering::Greater => Some(selection - 1),
                });
        }
        start
    }

    pub fn finish_change(&mut self, id: &MailId, succeeded: bool) -> ChangeCompletion {
        use remote::Change;

        let Some(index) = self
            .pending_changes
            .iter()
            .position(|pending| pending.original.id == *id)
        else {
            return ChangeCompletion::default();
        };
        let mut pending = self.pending_changes.remove(index);
        let move_failed = matches!(pending.change, Change::Move(_)) && !succeeded;
        if succeeded {
            match pending.change {
                Change::Seen(seen) => pending.original.unread = !seen,
                Change::Flagged(flagged) => pending.original.flagged = flagged,
                Change::Move(_) => return ChangeCompletion::default(),
            }
        }

        // Skip queued toggles already satisfied by the last confirmed server state.
        let next = pending
            .queued
            .iter()
            .position(|change| change.affects(&pending.original))
            .map(|index| {
                pending
                    .queued
                    .drain(..=index)
                    .last()
                    .expect("queued change")
            });
        if let Some(change) = next {
            pending.change = change;
        }

        let mut restored_selection = false;
        if pending.folder == self.folder {
            let selection = self.selected().map(|mail| mail.id.clone());
            if let Some(index) = self.message_index(id) {
                let mail = &mut self.contents.messages[index];
                mail.unread = pending.original.unread;
                mail.flagged = pending.original.flagged;
            } else if move_failed {
                self.contents.messages.push(pending.original.clone());
                self.contents.sort();
            }

            if next.is_some() {
                pending.apply(&mut self.contents.messages);
            }
            let restore = move_failed && pending.was_selected && selection.is_none();
            let selected = selection.as_ref().or(restore.then_some(id));
            self.selected = selected.and_then(|id| self.message_index(id));
            restored_selection = restore && self.selected.is_some();
        }
        if next.is_some() {
            self.pending_changes.push(pending);
        }
        ChangeCompletion {
            restored_selection,
            next,
        }
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

    pub fn selection_for(&self, query: &str, drafts: usize) -> Option<usize> {
        if self.selected.is_none()
            && self.pending_changes.iter().any(|pending| {
                pending.folder == self.folder
                    && pending.was_selected
                    && (matches!(pending.change, remote::Change::Move(_))
                        || pending
                            .queued
                            .iter()
                            .any(|change| matches!(change, remote::Change::Move(_))))
            })
        {
            return None;
        }
        let mut visible = self.visible_messages(query, drafts);
        let first = visible.next()?.0;
        self.selected
            .filter(|&id| id == first || visible.any(|(visible_id, _)| visible_id == id))
            .or(Some(first))
    }

    pub fn remember_scroll(&mut self, offset: f32) {
        self.list_offset = offset;
    }

    pub fn restore_list_position(&mut self, query: &str, drafts: usize) -> cosmic::Task<Message> {
        let scroll = self.scroll_to(self.list_offset);
        let focus = self
            .visible_messages(query, drafts)
            .position(|(index, _)| Some(index) == self.selected)
            .map_or_else(cosmic::Task::none, |position| {
                widget::button::focus(self.row_ids[position].clone())
            });
        cosmic::Task::batch([scroll, focus])
    }

    fn page(&self, query: &str, drafts: usize) -> Page {
        let drafts = if self.folder == Folder::Drafts {
            drafts
        } else {
            0
        };
        let messages = if matches!(self.status, Status::Loaded) {
            if query.trim().is_empty() {
                self.contents.messages.len()
            } else {
                self.matching_messages(query).count()
            }
        } else {
            0
        };
        let total = drafts + messages;
        let count = total.div_ceil(MESSAGES_PER_PAGE).max(1);
        let index = self.page.min(count - 1);
        let start = index * MESSAGES_PER_PAGE;
        let end = (start + MESSAGES_PER_PAGE).min(total);
        Page {
            index,
            count,
            drafts: start.min(drafts)..end.min(drafts),
            messages: start.saturating_sub(drafts)..end.saturating_sub(drafts),
        }
    }

    fn visible_messages<'a>(
        &'a self,
        query: &'a str,
        drafts: usize,
    ) -> impl Iterator<Item = (usize, &'a Mail)> {
        let range = self.page(query, drafts).messages;
        self.matching_messages(query)
            .skip(range.start)
            .take(range.len())
    }

    pub fn turn_page(
        &mut self,
        query: &str,
        drafts: usize,
        direction: isize,
    ) -> Option<cosmic::Task<Message>> {
        let page = self.page(query, drafts);
        let next = page
            .index
            .saturating_add_signed(direction)
            .min(page.count - 1);
        if next == page.index {
            return None;
        }
        self.page = next;
        Some(self.scroll_to(0.0))
    }

    pub fn reset_page(&mut self) -> cosmic::Task<Message> {
        self.page = 0;
        self.scroll_to(0.0)
    }

    pub fn visible_sender_addresses<'a>(
        &'a self,
        query: &'a str,
        drafts: usize,
    ) -> impl Iterator<Item = &'a str> {
        self.visible_messages(query, drafts)
            .map(|(_, mail)| mail.address.as_str())
            .chain(self.selected().map(|mail| mail.address.as_str()))
    }

    pub fn set_sort_order(&mut self, order: SortOrder) -> cosmic::Task<Message> {
        if self.sort_order == order {
            return cosmic::Task::none();
        }
        self.sort_order = order;
        self.reset_page()
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

    pub fn step_selection(
        &mut self,
        query: &str,
        drafts: usize,
        step: isize,
    ) -> (Option<usize>, cosmic::Task<Message>) {
        let page = self.page(query, drafts);
        let Some(position) = self
            .matching_messages(query)
            .position(|(id, _)| Some(id) == self.selected)
            .filter(|position| page.messages.contains(position))
        else {
            return (self.selection_for(query, drafts), cosmic::Task::none());
        };
        let position = position.saturating_add_signed(step);
        let Some((id, _)) = self.matching_messages(query).nth(position) else {
            return (self.selected, cosmic::Task::none());
        };
        let drafts = if self.folder == Folder::Drafts {
            drafts
        } else {
            0
        };
        self.page = (position + drafts) / MESSAGES_PER_PAGE;
        let scroll = match self.page.cmp(&page.index) {
            std::cmp::Ordering::Less => self.scroll_to(f32::MAX),
            std::cmp::Ordering::Greater => self.scroll_to(0.0),
            std::cmp::Ordering::Equal => cosmic::Task::none(),
        };
        (Some(id), scroll)
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

        assert_eq!(
            mailbox.begin_change(&first.id, Change::Seen(true)),
            ChangeStart::Send
        );
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
        assert_eq!(
            mailbox.begin_change(&second.id, Change::Flagged(true)),
            ChangeStart::Send
        );
        mailbox.finish_change(&first.id, false);
        assert!(mailbox.contents.messages[0].unread);
        assert!(mailbox.selected().unwrap().flagged);
        mailbox.finish_change(&second.id, true);

        mailbox.set_selection(Some(0));
        assert_eq!(
            mailbox.begin_change(&first.id, Change::Move(Folder::Archive)),
            ChangeStart::Send
        );
        assert_eq!(mailbox.contents.messages.len(), 1);
        mailbox.finish_load(mailbox.request, Some(server), true);
        assert_eq!(mailbox.selection_for("", 0), None);
        assert!(mailbox.finish_change(&first.id, false).restored_selection);
        assert_eq!(mailbox.selected().unwrap().id, first.id);
        assert_eq!(mailbox.contents.messages.len(), 2);

        mailbox.set_selection(Some(1));
        assert_eq!(
            mailbox.begin_change(&first.id, Change::Move(Folder::Trash)),
            ChangeStart::Send
        );
        assert_eq!(mailbox.selected().unwrap().id, second.id);
        assert!(!mailbox.finish_change(&first.id, false).restored_selection);
        assert_eq!(mailbox.selected().unwrap().id, second.id);
    }

    #[test]
    fn queued_mail_actions_preserve_latest_intent_and_rollback_independently() {
        use remote::Change;

        let mut original = mail(1, "Queued message");
        original.unread = true;
        let id = original.id.clone();
        let mut mailbox = Mailbox::new();
        mailbox.contents.messages.push(original.clone());
        mailbox.set_selection(Some(0));

        assert_eq!(
            mailbox.begin_change(&id, Change::Seen(true)),
            ChangeStart::Send
        );
        assert_eq!(
            mailbox.begin_change(&id, Change::Seen(false)),
            ChangeStart::Queued
        );
        assert!(mailbox.selected().unwrap().unread);
        assert_eq!(
            mailbox.begin_change(&id, Change::Flagged(true)),
            ChangeStart::Queued
        );
        assert!(mailbox.selected().unwrap().flagged);
        assert!(matches!(
            mailbox.finish_change(&id, true).next,
            Some(Change::Seen(false))
        ));
        assert!(mailbox.selected().unwrap().unread);

        assert_eq!(
            mailbox.begin_change(&id, Change::Move(Folder::Archive)),
            ChangeStart::Queued
        );
        let stale = remote::FolderContents {
            messages: vec![original],
            ..Default::default()
        };
        mailbox.finish_load(mailbox.request, Some(stale), true);
        assert!(mailbox.contents.messages.is_empty());
        assert!(matches!(
            mailbox.finish_change(&id, false).next,
            Some(Change::Flagged(true))
        ));
        assert!(matches!(
            mailbox.finish_change(&id, true).next,
            Some(Change::Move(Folder::Archive))
        ));
        let finished = mailbox.finish_change(&id, false);
        assert!(finished.restored_selection);
        assert!(finished.next.is_none());
        let restored = mailbox.selected().unwrap();
        assert!(!restored.unread);
        assert!(restored.flagged);

        assert_eq!(
            mailbox.begin_change(&id, Change::Flagged(false)),
            ChangeStart::Send
        );
        assert_eq!(
            mailbox.begin_change(&id, Change::Flagged(true)),
            ChangeStart::Queued
        );
        assert!(mailbox.finish_change(&id, false).next.is_none());
        assert!(mailbox.selected().unwrap().flagged);

        // A sync already in flight can predate a confirmed queued change.
        let stale = mailbox.contents.clone();
        assert_eq!(
            mailbox.begin_change(&id, Change::Seen(false)),
            ChangeStart::Send
        );
        assert_eq!(
            mailbox.begin_change(&id, Change::Flagged(false)),
            ChangeStart::Queued
        );
        assert!(matches!(
            mailbox.finish_change(&id, true).next,
            Some(Change::Flagged(false))
        ));
        mailbox.finish_load(mailbox.request, Some(stale), true);
        assert!(mailbox.selected().unwrap().unread);
        assert!(!mailbox.selected().unwrap().flagged);
        mailbox.finish_change(&id, true);
    }

    #[test]
    fn pagination_reaches_all_matches_and_steps_across_pages() {
        let mut mailbox = Mailbox::new();
        mailbox.status = Status::Loaded;
        mailbox.contents.messages = (0..(MESSAGES_PER_PAGE * 4 + 2))
            .map(|index| {
                let subject = if index % 2 == 0 { "Match" } else { "Other" };
                mail(index as u32 + 1, subject)
            })
            .collect();

        let query = "Match";
        assert_eq!(mailbox.page(query, 0).count, 3);
        assert_eq!(
            mailbox.visible_messages(query, 0).count(),
            MESSAGES_PER_PAGE
        );
        assert!(mailbox.set_selection(Some((MESSAGES_PER_PAGE - 1) * 2)));
        let (selection, _) = mailbox.step_selection(query, 0, 1);
        assert_eq!(selection, Some(MESSAGES_PER_PAGE * 2));
        assert_eq!(mailbox.page(query, 0).index, 1);
        assert!(mailbox.set_selection(selection));
        let (selection, _) = mailbox.step_selection(query, 0, -1);
        assert_eq!(selection, Some((MESSAGES_PER_PAGE - 1) * 2));
        assert_eq!(mailbox.page(query, 0).index, 0);

        assert!(mailbox.turn_page(query, 0, 1).is_some());
        assert!(mailbox.turn_page(query, 0, 1).is_some());
        assert!(mailbox.turn_page(query, 0, 1).is_none());
        assert_eq!(mailbox.visible_messages(query, 0).count(), 1);
        mailbox.contents.messages.truncate(MESSAGES_PER_PAGE);
        assert_eq!(mailbox.page(query, 0).index, 0);
    }

    #[test]
    fn local_drafts_and_server_messages_share_the_page_limit() {
        let mut mailbox = Mailbox::new();
        mailbox.folder = Folder::Drafts;
        mailbox.status = Status::Loaded;
        mailbox.contents.messages = vec![mail(1, "Server draft")];
        let drafts = MESSAGES_PER_PAGE + 1;

        let page = mailbox.page("", drafts);
        assert_eq!(page.count, 2);
        assert_eq!(page.drafts.len(), MESSAGES_PER_PAGE);
        assert!(page.messages.is_empty());
        assert!(mailbox.turn_page("", drafts, 1).is_some());

        let page = mailbox.page("", drafts);
        assert_eq!(page.drafts, MESSAGES_PER_PAGE..drafts);
        assert_eq!(page.messages, 0..1);
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
        let mut state = Tree::new(mailbox.view("", &[], false, false, None, None).as_widget());

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
                    .view(query, &[], false, pictures, None, None)
                    .as_widget_mut(),
            );
            tree::NAMED.with(|named| named.borrow_mut().clear());

            assert!(
                mailbox
                    .visible_messages(query, 0)
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
