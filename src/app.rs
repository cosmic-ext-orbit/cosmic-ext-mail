// SPDX-License-Identifier: GPL-3.0-only

use crate::features::{
    composer::{Composer, ComposerMessage},
    mailbox::{ListMessage, Mailbox},
    reader::{Reader, ReaderMessage},
    search::{Search, SearchMessage},
};
use crate::fl;
use cosmic::app::Task;
use cosmic::iced::{Event, Length, Subscription, event, keyboard, window};
use cosmic::prelude::*;
use cosmic::widget::{self, nav_bar, pane_grid};

const SIDEBAR_COLLAPSE_WIDTH: f32 = 900.0;
const SINGLE_PANE_WIDTH: f32 = 600.0;

#[derive(Clone, Copy)]
enum Pane {
    Messages,
    Reader,
}

#[derive(Clone, Debug)]
pub enum Message {
    Search(SearchMessage),
    List(ListMessage),
    Reader(ReaderMessage),
    Composer(ComposerMessage),
    ResizePanes(pane_grid::ResizeEvent),
}

pub struct AppModel {
    core: cosmic::Core,
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
        let reader = Reader::new(mailbox.selected());
        let (mut panes, list) = pane_grid::State::new(Pane::Messages);
        let (_, divider) = panes
            .split(pane_grid::Axis::Vertical, list, Pane::Reader)
            .expect("the message list pane exists");
        panes.resize(divider, 0.34);

        let mut app = Self {
            core,
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
        (app, title)
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        vec![self.composer.new_message_button().map(Message::Composer)]
    }

    fn header_center(&self) -> Vec<Element<'_, Message>> {
        vec![self.search.view(self.narrow).map(Message::Search)]
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(self.mailbox.nav_model())
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<Message> {
        if !self.mailbox.select_folder(id) {
            return Task::none();
        }
        self.search.clear();
        self.refresh_list()
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
            Message::Search(message) => return self.update_search(message),
            Message::List(message) => self.update_list(message),
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

        let mut page = widget::column::with_capacity(2)
            .push(content)
            .height(Length::Fill);
        if let Some(undo) = self.mailbox.undo_view() {
            page = page.push(undo.map(Message::List));
        }
        page.into()
    }

    fn on_escape(&mut self) -> Task<Message> {
        if self.composer.is_open() {
            self.composer.update(ComposerMessage::Save);
            return Task::none();
        }
        self.update_reader(ReaderMessage::BackToList)
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
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

    fn update_list(&mut self, message: ListMessage) {
        match message {
            ListMessage::Select(id) => {
                self.select(Some(id));
                self.open_selected();
            }
            ListMessage::Step(direction) => {
                self.select(self.mailbox.step_selection(self.search.query(), direction));
            }
            ListMessage::OpenSelected => self.open_selected(),
            ListMessage::Scrolled(offset) => self.mailbox.remember_scroll(offset),
            ListMessage::ResumeDraft => self.composer.update(ComposerMessage::Open),
            ListMessage::UndoMove => {
                if self.mailbox.undo_move() {
                    self.select(self.mailbox.selection_for(self.search.query()));
                }
            }
        }
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
            ReaderMessage::ToggleUnread => self.mailbox.toggle_selected_unread(),
            ReaderMessage::MoveTo(folder) => {
                let next = self.mailbox.move_selected(folder, self.search.query());
                self.select(next);
            }
            ReaderMessage::Reply => self.composer.reply(self.mailbox.selected()),
        }
        Task::none()
    }

    fn refresh_list(&mut self) -> Task<Message> {
        self.reader.set_open(false);
        self.select(self.mailbox.selection_for(self.search.query()));
        self.mailbox
            .scroll_to(0.0)
            .map(|message| Message::List(message).into())
    }

    fn select(&mut self, selection: Option<usize>) {
        if self.mailbox.set_selection(selection) {
            self.reader.set_message(self.mailbox.selected());
        }
    }

    fn open_selected(&mut self) {
        if self.mailbox.selected().is_some() {
            self.reader.set_open(true);
            self.mailbox.mark_selected_read();
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
        keyboard::Key::Character("f") if modifiers.control() => {
            Some(Message::Search(SearchMessage::Focus))
        }
        keyboard::Key::Named(Named::ArrowUp) => Some(Message::List(ListMessage::Step(-1))),
        keyboard::Key::Named(Named::ArrowDown) => Some(Message::List(ListMessage::Step(1))),
        keyboard::Key::Named(Named::Enter) => Some(Message::List(ListMessage::OpenSelected)),
        _ => None,
    }
}
