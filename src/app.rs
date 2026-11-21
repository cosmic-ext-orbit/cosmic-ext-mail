// SPDX-License-Identifier: GPL-3.0-only

use crate::features::{
    accounts::{self, Accounts},
    composer::{Composer, ComposerMessage},
    mailbox::{ListMessage, Mailbox},
    reader::{Reader, ReaderMessage},
    search::{Search, SearchMessage},
};
use crate::fl;
use cosmic::app::Task;
use cosmic::iced::{Event, Length, Subscription, event, keyboard, window};
use cosmic::prelude::*;
use cosmic::widget::{nav_bar, pane_grid};

const SIDEBAR_COLLAPSE_WIDTH: f32 = 900.0;
const SINGLE_PANE_WIDTH: f32 = 600.0;

#[derive(Clone, Copy)]
enum Pane {
    Messages,
    Reader,
}

#[derive(Clone, Debug)]
pub enum Message {
    Accounts(accounts::Message),
    Search(SearchMessage),
    List(ListMessage),
    Reader(ReaderMessage),
    Composer(ComposerMessage),
    ResizePanes(pane_grid::ResizeEvent),
}

pub struct AppModel {
    core: cosmic::Core,
    accounts: Accounts,
    mailbox: Mailbox,
    reader: Reader,
    search: Search,
    composer: Composer,
    panes: pane_grid::State<Pane>,
    narrow: bool,
    compact_sidebar: bool,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "dev.edfloreshz.Mail";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _: ()) -> (Self, Task<Message>) {
        let mailbox = Mailbox::new();
        let reader = Reader::new();
        let (mut panes, list) = pane_grid::State::new(Pane::Messages);
        let (_, divider) = panes
            .split(pane_grid::Axis::Vertical, list, Pane::Reader)
            .expect("the message list pane exists");
        panes.resize(divider, 0.34);

        let mut app = Self {
            core,
            accounts: Accounts::new(),
            mailbox,
            reader,
            search: Search::new(),
            composer: Composer::default(),
            panes,
            narrow: false,
            compact_sidebar: false,
        };
        let title = app
            .core
            .main_window_id()
            .map_or_else(Task::none, |id| app.set_window_title(fl!("app-title"), id));
        let load = app.reload_mail();
        (app, Task::batch([title, load]))
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        vec![self.composer.new_message_button().map(Message::Composer)]
    }

    fn header_center(&self) -> Vec<Element<'_, Message>> {
        vec![self.search.view(self.narrow).map(Message::Search)]
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        vec![self.accounts.button().map(Message::Accounts)]
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(self.mailbox.nav_model())
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        if !self.mailbox.select_folder(id) {
            return Task::none();
        }
        self.search.clear();
        self.reload_mail()
    }

    fn on_window_resize(&mut self, id: window::Id, width: f32, _: f32) {
        if !self.core.main_window_is(id) {
            return;
        }
        let compact = width < SIDEBAR_COLLAPSE_WIDTH;
        if compact != self.compact_sidebar {
            self.core.nav_bar_set_toggled(!compact);
            self.compact_sidebar = compact;
        }
        self.narrow = width < SINGLE_PANE_WIDTH;
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            self.reader.subscription().map(Message::Reader),
            event::listen_with(shortcut),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Accounts(message) => {
                let reload = matches!(
                    &message,
                    accounts::Message::Finished(Ok(
                        accounts::Outcome::Saved(_) | accounts::Outcome::Removed(_)
                    ))
                );
                let update = self
                    .accounts
                    .update(message)
                    .map(|message| Message::Accounts(message).into());
                return if reload {
                    Task::batch([update, self.reload_mail()])
                } else {
                    update
                };
            }
            Message::Search(message) => return self.update_search(message),
            Message::List(message) => return self.update_list(message),
            Message::Reader(message) => return self.update_reader(message),
            Message::Composer(message) => self.composer.update(message),
            Message::ResizePanes(event) => self.panes.resize(event.split, event.ratio),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let list = || {
            self.mailbox
                .view(self.search.query(), self.composer.has_draft())
                .map(Message::List)
        };
        let reader = || {
            self.reader
                .view(self.mailbox.selected(), self.narrow)
                .map(Message::Reader)
        };
        let content: Element<_> = if self.narrow {
            if self.reader.is_open() {
                reader()
            } else {
                list()
            }
        } else {
            pane_grid::PaneGrid::new(&self.panes, |_, pane, _| {
                pane_grid::Content::new(match pane {
                    Pane::Messages => list(),
                    Pane::Reader => reader(),
                })
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(5)
            .min_size(200)
            .on_resize(8, Message::ResizePanes)
            .into()
        };

        content
    }

    fn on_escape(&mut self) -> Task<Message> {
        if self.accounts.is_open() {
            return self
                .accounts
                .update(accounts::Message::Close)
                .map(|message| Message::Accounts(message).into());
        }
        if self.composer.is_open() {
            self.composer.update(ComposerMessage::Save);
            return Task::none();
        }
        self.update_reader(ReaderMessage::BackToList)
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        if let Some(dialog) = self.accounts.dialog() {
            return Some(dialog.map(Message::Accounts));
        }
        self.composer
            .dialog()
            .map(|dialog| dialog.map(Message::Composer))
    }
}

impl AppModel {
    fn update_search(&mut self, message: SearchMessage) -> Task<Message> {
        match message {
            SearchMessage::Changed(query) => {
                self.search.set_query(query);
                self.refresh_list()
            }
            SearchMessage::Focus => self
                .search
                .focus()
                .map(|message| Message::Search(message).into()),
            SearchMessage::Submit => {
                self.open_selected();
                Task::none()
            }
        }
    }

    fn update_list(&mut self, message: ListMessage) -> Task<Message> {
        match message {
            ListMessage::Select(id) => {
                let task = self.select(Some(id));
                self.open_selected();
                return task;
            }
            ListMessage::Step(direction) => {
                return self.select(self.mailbox.step_selection(self.search.query(), direction));
            }
            ListMessage::OpenSelected => self.open_selected(),
            ListMessage::Scrolled(offset) => self.mailbox.remember_scroll(offset),
            ListMessage::ResumeDraft => self.composer.update(ComposerMessage::Open),
            ListMessage::Refresh => return self.reload_mail(),
            ListMessage::AddAccount => {
                return self
                    .accounts
                    .update(accounts::Message::Add)
                    .map(|message| Message::Accounts(message).into());
            }
            ListMessage::Download(request, event) => self.mailbox.update_download(request, event),
            ListMessage::Cached(request, contents) => {
                if self.mailbox.finish_load(request, contents, false).is_some() {
                    return self.select(self.mailbox.selection_for(self.search.query()));
                }
            }
            ListMessage::Loaded(request, contents) => {
                if let Some(selection_removed) =
                    self.mailbox.finish_load(request, Some(contents), true)
                {
                    let selection = self.mailbox.selection_for(self.search.query());
                    if selection_removed && selection.is_none() {
                        self.reader.set_open(false);
                        return self.load_body();
                    }
                    return self.select(selection);
                }
            }
        }
        Task::none()
    }

    fn update_reader(&mut self, message: ReaderMessage) -> Task<Message> {
        match message {
            ReaderMessage::Wake => {}
            ReaderMessage::BackToList => {
                self.reader.set_open(false);
                return self
                    .mailbox
                    .restore_list_position()
                    .map(|message| Message::List(message).into());
            }
            ReaderMessage::ShowRemoteImages => self.reader.show_remote_images(),
            ReaderMessage::Retry => return self.load_body(),
            ReaderMessage::Loaded(request, result) => self.reader.finish_load(request, result),
            ReaderMessage::Reply => self.composer.reply(self.mailbox.selected()),
        }
        Task::none()
    }

    fn reload_mail(&mut self) -> Task<Message> {
        let reload = self.mailbox.reload(&self.accounts);
        self.reader.set_open(false);
        Task::batch([
            reload.map(|message| Message::List(message).into()),
            self.load_body(),
            self.mailbox
                .scroll_to(0.0)
                .map(|message| Message::List(message).into()),
        ])
    }

    fn refresh_list(&mut self) -> Task<Message> {
        self.reader.set_open(false);
        Task::batch([
            self.select(self.mailbox.selection_for(self.search.query())),
            self.mailbox
                .scroll_to(0.0)
                .map(|message| Message::List(message).into()),
        ])
    }

    fn select(&mut self, selection: Option<usize>) -> Task<Message> {
        if self.mailbox.set_selection(selection) {
            self.load_body()
        } else {
            Task::none()
        }
    }

    fn load_body(&mut self) -> Task<Message> {
        self.reader
            .load(self.mailbox.selected(), &self.accounts)
            .map(|message| Message::Reader(message).into())
    }

    fn open_selected(&mut self) {
        if self.mailbox.selected().is_some() {
            self.reader.set_open(true);
        }
    }
}

fn shortcut(event: Event, status: event::Status, _: window::Id) -> Option<Message> {
    use keyboard::key::Named;

    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    if status == event::Status::Captured {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character(",") if modifiers.control() => {
            Some(Message::Accounts(accounts::Message::Open))
        }
        keyboard::Key::Character("f") if modifiers.control() => {
            Some(Message::Search(SearchMessage::Focus))
        }
        keyboard::Key::Named(Named::ArrowUp) => Some(Message::List(ListMessage::Step(-1))),
        keyboard::Key::Named(Named::ArrowDown) => Some(Message::List(ListMessage::Step(1))),
        keyboard::Key::Named(Named::Enter) => Some(Message::List(ListMessage::OpenSelected)),
        _ => None,
    }
}
