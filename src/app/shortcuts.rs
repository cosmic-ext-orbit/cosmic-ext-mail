// SPDX-License-Identifier: GPL-3.0-only

use super::{ContextPage, MenuAction};
use crate::{features::mailbox::Folder, fl};
use cosmic::Element;
use cosmic::iced::keyboard::{Key, Modifiers, key::Named};
use cosmic::iced::{Event, Subscription, event, keyboard, window};
use cosmic::widget::{self, menu};
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Scope {
    Mail,
    Composer,
    Account,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Menu(MenuAction),
    NewMessage,
    Search,
    Refresh,
    AddAccount,
    CloseWindow,
    Folder(Folder),
    Step(isize),
    OpenMessage,
    Reply,
    Forward,
    ToggleRead,
    ToggleStar,
    Move(Folder),
    CopySender,
    Send,
    Attach,
    SaveDraft,
    DiscardDraft,
    CloseDialog,
    TestAccount,
    SaveAccount,
}

struct Binding {
    scope: Scope,
    action: Action,
    key: menu::KeyBind,
}

static BINDINGS: LazyLock<Vec<Binding>> = LazyLock::new(|| {
    use crate::features::mailbox::Folder as MailFolder;
    use Action::*;
    use Scope::{Account, Composer, Mail};
    use menu::key_bind::Modifier::{self, Alt, Ctrl, Shift};

    let binding = |scope, action, modifiers: &[Modifier], key| Binding {
        scope,
        action,
        key: menu::KeyBind {
            modifiers: modifiers.to_vec(),
            key,
        },
    };
    let character = |key: &'static str| Key::Character(key.into());
    let mut bindings = vec![
        binding(Mail, NewMessage, &[Ctrl], character("n")),
        binding(Mail, Search, &[Ctrl], character("f")),
        binding(Mail, Refresh, &[Ctrl], character("r")),
        binding(Mail, Refresh, &[], Key::Named(Named::F5)),
        binding(Mail, AddAccount, &[Ctrl, Shift], character("a")),
        binding(
            Mail,
            Menu(MenuAction::Open(ContextPage::Settings)),
            &[Ctrl],
            character(","),
        ),
        binding(
            Mail,
            Menu(MenuAction::Open(ContextPage::Shortcuts)),
            &[],
            Key::Named(Named::F1),
        ),
        binding(
            Mail,
            Menu(MenuAction::Open(ContextPage::About)),
            &[Ctrl, Shift],
            character("b"),
        ),
        binding(Mail, CloseWindow, &[Ctrl], character("w")),
        binding(Mail, Menu(MenuAction::Quit), &[Ctrl], character("q")),
        binding(Mail, Step(-1), &[], Key::Named(Named::ArrowUp)),
        binding(Mail, Step(1), &[], Key::Named(Named::ArrowDown)),
        binding(Mail, OpenMessage, &[], Key::Named(Named::Enter)),
        binding(Mail, Reply, &[Ctrl, Shift], character("r")),
        binding(Mail, Forward, &[Ctrl, Shift], character("f")),
        binding(Mail, ToggleRead, &[Ctrl], character("u")),
        binding(Mail, ToggleStar, &[Ctrl, Shift], character("s")),
        binding(Mail, Move(MailFolder::Archive), &[Ctrl], character("e")),
        binding(Mail, Move(MailFolder::Junk), &[Ctrl, Shift], character("j")),
        binding(
            Mail,
            Move(MailFolder::Trash),
            &[],
            Key::Named(Named::Delete),
        ),
        binding(
            Mail,
            Move(MailFolder::Inbox),
            &[Ctrl, Shift],
            character("i"),
        ),
        binding(Mail, CopySender, &[Ctrl, Shift], character("c")),
        binding(Composer, Send, &[Ctrl], Key::Named(Named::Enter)),
        binding(Composer, Attach, &[Ctrl, Shift], character("a")),
        binding(Composer, SaveDraft, &[Ctrl], character("s")),
        binding(
            Composer,
            DiscardDraft,
            &[Ctrl, Shift],
            Key::Named(Named::Delete),
        ),
        binding(Composer, CloseDialog, &[Ctrl], character("w")),
        binding(Account, TestAccount, &[Ctrl], character("t")),
        binding(Account, SaveAccount, &[Ctrl], Key::Named(Named::Enter)),
        binding(Account, CloseDialog, &[Ctrl], character("w")),
    ];
    for (folder, key) in MailFolder::ALL
        .into_iter()
        .zip(["1", "2", "3", "4", "5", "6"])
    {
        bindings.push(binding(Mail, Folder(folder), &[Alt], character(key)));
    }
    bindings
});

impl Action {
    fn label(self) -> String {
        match self {
            Self::Menu(MenuAction::Open(ContextPage::Settings)) => fl!("settings"),
            Self::Menu(MenuAction::Open(ContextPage::About)) => fl!("about"),
            Self::Menu(MenuAction::Open(ContextPage::Shortcuts)) => fl!("keyboard-shortcuts"),
            Self::Menu(MenuAction::Quit) => fl!("mail-quit"),
            Self::NewMessage => fl!("new-message"),
            Self::Search => fl!("search-mail"),
            Self::Refresh => fl!("mail-refresh"),
            Self::AddAccount => fl!("account-add"),
            Self::CloseWindow | Self::CloseDialog => fl!("shortcut-close"),
            Self::Folder(folder) => folder.label(),
            Self::Step(direction) if direction < 0 => fl!("shortcut-previous-message"),
            Self::Step(_) => fl!("shortcut-next-message"),
            Self::OpenMessage => fl!("mail-open-message"),
            Self::Reply => fl!("reply"),
            Self::Forward => fl!("forward"),
            Self::ToggleRead => fl!("shortcut-toggle-read"),
            Self::ToggleStar => fl!("shortcut-toggle-star"),
            Self::Move(Folder::Archive) => fl!("archive-message"),
            Self::Move(Folder::Junk) => fl!("spam-message"),
            Self::Move(Folder::Inbox) => fl!("mail-move-to-inbox"),
            Self::Move(_) => fl!("delete-message"),
            Self::CopySender => fl!("copy-email-address"),
            Self::Send => fl!("send-message"),
            Self::Attach => fl!("attach-files"),
            Self::SaveDraft => fl!("save-draft"),
            Self::DiscardDraft => fl!("discard-draft"),
            Self::TestAccount => fl!("account-check"),
            Self::SaveAccount => fl!("account-save"),
        }
    }
}

pub(super) fn menu_bindings() -> HashMap<menu::KeyBind, MenuAction> {
    BINDINGS
        .iter()
        .filter_map(|binding| match binding.action {
            Action::Menu(action) => Some((binding.key.clone(), action)),
            _ => None,
        })
        .collect()
}

pub(super) fn subscription(main_window: Option<window::Id>, scope: Scope) -> Subscription<Action> {
    event::listen_with(|event, status, id| {
        if let Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            repeat,
            ..
        }) = event
        {
            Some((id, key, physical_key, modifiers, repeat, status))
        } else {
            None
        }
    })
    .with((main_window, scope))
    .filter_map(
        |((main_window, scope), (id, key, physical, modifiers, repeat, status))| {
            if main_window != Some(id) {
                return None;
            }
            resolve(scope, &key, Some(&physical), modifiers, repeat, status)
        },
    )
}

fn resolve(
    scope: Scope,
    key: &Key,
    physical: Option<&keyboard::key::Physical>,
    modifiers: Modifiers,
    repeat: bool,
    status: event::Status,
) -> Option<Action> {
    if scope == Scope::Mail && status == event::Status::Captured {
        return None;
    }
    BINDINGS.iter().find_map(|binding| {
        (binding.scope == scope
            && (!repeat || matches!(binding.action, Action::Step(_)))
            && binding.key.matches(modifiers, key, physical))
        .then_some(binding.action)
    })
}

pub(super) fn view<Message: Clone + 'static>() -> Element<'static, Message> {
    widget::column::with_capacity(3)
        .spacing(12)
        .push(section(Scope::Mail, fl!("app-title")))
        .push(section(Scope::Composer, fl!("new-message")))
        .push(section(Scope::Account, fl!("account-add")))
        .into()
}

fn section<Message: Clone + 'static>(scope: Scope, title: String) -> Element<'static, Message> {
    let mut section = widget::settings::section().title(title);
    for binding in BINDINGS.iter().filter(|binding| binding.scope == scope) {
        section = section.add(
            widget::settings::item::builder(binding.action.label())
                .control(widget::text::caption(binding.key.to_string())),
        );
    }
    section.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_and_repeated_delete_preserve_the_message() {
        let key = Key::Named(Named::Delete);
        let action =
            |repeat, status| resolve(Scope::Mail, &key, None, Modifiers::empty(), repeat, status);

        assert_eq!(action(false, event::Status::Captured), None);
        assert_eq!(action(true, event::Status::Ignored), None);
        assert_eq!(
            action(false, event::Status::Ignored),
            Some(Action::Move(Folder::Trash))
        );
    }

    #[test]
    fn control_enter_uses_the_open_dialog() {
        let key = Key::Named(Named::Enter);
        let action = |scope| {
            resolve(
                scope,
                &key,
                None,
                Modifiers::CTRL,
                false,
                event::Status::Captured,
            )
        };

        assert_eq!(action(Scope::Mail), None);
        assert_eq!(action(Scope::Composer), Some(Action::Send));
        assert_eq!(action(Scope::Account), Some(Action::SaveAccount));
    }
}
