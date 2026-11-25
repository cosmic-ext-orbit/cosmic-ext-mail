// SPDX-License-Identifier: GPL-3.0-only

use super::{Folder, Mail, MailId, Mailbox, Message, remote};
use crate::features::accounts::Accounts;
use cosmic::Task;

#[derive(Clone, Copy, Debug)]
pub enum Change {
    Move(Folder),
    DeletePermanently,
    Seen(bool),
    Flagged(bool),
}

impl Change {
    pub fn removes_message(self) -> bool {
        matches!(self, Self::Move(_) | Self::DeletePermanently)
    }

    pub(super) fn affects(self, mail: &Mail) -> bool {
        match self {
            Self::Move(_) | Self::DeletePermanently => true,
            Self::Seen(seen) => mail.unread == seen,
            Self::Flagged(flagged) => mail.flagged != flagged,
        }
    }

    pub(super) fn apply(self, messages: &mut Vec<Mail>, id: &MailId) {
        let Some(index) = messages.iter().position(|mail| mail.id == *id) else {
            return;
        };
        match self {
            Self::Move(_) | Self::DeletePermanently => {
                messages.remove(index);
            }
            Self::Seen(seen) => messages[index].unread = !seen,
            Self::Flagged(flagged) => messages[index].flagged = flagged,
        }
    }
}

pub(super) struct PendingChange {
    pub(super) folder: Folder,
    original: Mail,
    change: Change,
    queued: Vec<Change>,
    was_selected: bool,
}

impl PendingChange {
    pub(super) fn apply(&self, messages: &mut Vec<Mail>) {
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
pub(super) enum ChangeStart {
    Send,
    Queued,
    Unavailable,
}

#[derive(Default)]
pub(super) struct ChangeCompletion {
    pub restored_selection: bool,
    pub next: Option<Change>,
}

impl PendingChange {
    pub(super) fn removes_selection(&self, folder: Folder) -> bool {
        self.folder == folder
            && self.was_selected
            && (self.change.removes_message()
                || self.queued.iter().any(|change| change.removes_message()))
    }
}

pub struct ActionOutcome {
    pub task: Task<Message>,
    pub error: Option<remote::Error>,
    pub reload_reader: bool,
}

impl Mailbox {
    pub(super) fn begin_change(&mut self, id: &MailId, change: Change) -> ChangeStart {
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
        let moving = change.removes_message();
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
            self.invalidate_threads();
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

    pub(super) fn finish_change(&mut self, id: &MailId, succeeded: bool) -> ChangeCompletion {
        let Some(index) = self
            .pending_changes
            .iter()
            .position(|pending| pending.original.id == *id)
        else {
            return ChangeCompletion::default();
        };
        let mut pending = self.pending_changes.remove(index);
        let move_failed = pending.change.removes_message() && !succeeded;
        if succeeded {
            match pending.change {
                Change::Seen(seen) => pending.original.unread = !seen,
                Change::Flagged(flagged) => pending.original.flagged = flagged,
                Change::Move(_) | Change::DeletePermanently => return ChangeCompletion::default(),
            }
        }

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
                self.invalidate_threads();
            }

            if next.is_some() {
                let before = self.contents.messages.len();
                pending.apply(&mut self.contents.messages);
                if self.contents.messages.len() != before {
                    self.invalidate_threads();
                }
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

    pub fn request_change(
        &mut self,
        accounts: &Accounts,
        id: MailId,
        change: Change,
    ) -> (Task<Message>, bool) {
        let closes_reader =
            change.removes_message() && self.selected().is_some_and(|mail| mail.id == id);
        match self.begin_change(&id, change) {
            ChangeStart::Send => (
                Self::send_change(accounts, self.folder, id, change),
                closes_reader,
            ),
            ChangeStart::Queued => (Task::none(), closes_reader),
            ChangeStart::Unavailable => (Task::none(), false),
        }
    }

    pub fn complete_change(
        &mut self,
        accounts: &Accounts,
        folder: Folder,
        id: MailId,
        change: Change,
        result: Result<(), remote::Error>,
    ) -> ActionOutcome {
        let completion = self.finish_change(&id, result.is_ok());
        let error = result.err();
        let task = if let Some(next) = completion.next {
            Self::send_change(accounts, folder, id, next)
        } else if error.is_none()
            && (self.folder == folder
                || matches!(change, Change::Move(destination) if self.folder == destination))
        {
            self.refresh_after_change(accounts)
        } else {
            Task::none()
        };
        ActionOutcome {
            task,
            error,
            reload_reader: completion.restored_selection,
        }
    }

    fn send_change(
        accounts: &Accounts,
        folder: Folder,
        id: MailId,
        change: Change,
    ) -> Task<Message> {
        let account = accounts
            .usable_vec()
            .into_iter()
            .find(|account| account.id == id.account);
        let Some(account) = account else {
            return Task::done(Message::Changed(
                folder,
                id,
                change,
                Err(remote::Error::MessageUnavailable),
            ));
        };
        Task::perform(
            remote::change(account, folder, id.clone(), change),
            move |result| Message::Changed(folder, id, change, result),
        )
    }
}
