// SPDX-License-Identifier: GPL-3.0-only

use crate::config::{Config, MessageActions};
use crate::features::{
    accounts::{self, Accounts},
    composer::{self, Composer},
    mailbox::{self, Mailbox},
    reader::{self, Reader},
    search::{self, Search},
};
use crate::fl;
use cosmic::app::{Task, context_drawer};
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::{Event, Length, Subscription, event, keyboard, window};
use cosmic::prelude::*;
use cosmic::widget::{self, about::About, menu, nav_bar, pane_grid};
use std::collections::HashMap;

const SIDEBAR_COLLAPSE_WIDTH: f32 = 900.0;
const SINGLE_PANE_WIDTH: f32 = 600.0;

#[derive(Clone, Copy)]
enum Pane {
    Messages,
    Reader,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextPage {
    Settings,
    About,
}

impl menu::Action for ContextPage {
    type Message = Message;

    fn message(&self) -> Message {
        Message::ToggleContextPage(*self)
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Accounts(accounts::Message),
    Search(search::Message),
    List(mailbox::Message),
    Reader(reader::Message),
    Composer(composer::Message),
    ResizePanes(pane_grid::ResizeEvent),
    ToggleContextPage(ContextPage),
    CloseContext,
    LaunchUrl(String),
    SetLoadRemoteImages(bool),
    SetMessageActions(usize),
    ConfigUpdated(Config),
}

pub struct AppModel {
    core: cosmic::Core,
    about: About,
    context_page: ContextPage,
    config: Config,
    config_handler: Option<cosmic_config::Config>,
    config_error: bool,
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
        let (mut panes, list) = pane_grid::State::new(Pane::Messages);
        let (_, divider) = panes
            .split(pane_grid::Axis::Vertical, list, Pane::Reader)
            .expect("the message list pane exists");
        panes.resize(divider, 0.4);

        let config_handler = cosmic_config::Config::new(Self::APP_ID, Config::VERSION).ok();
        let (config, config_error) = match config_handler.as_ref().map(Config::get_entry) {
            Some(Ok(config)) => (config, false),
            Some(Err((_, config))) => (config, true),
            None => (Config::default(), true),
        };

        let mut app = Self {
            core,
            about: About::default()
                .name(fl!("app-title"))
                .comments(fl!("app-comment"))
                .icon(widget::icon::from_svg_bytes(include_bytes!(
                    "../resources/icons/hicolor/scalable/apps/icon.svg"
                )))
                .version(env!("CARGO_PKG_VERSION"))
                .license(env!("CARGO_PKG_LICENSE"))
                .links([(fl!("repository"), env!("CARGO_PKG_REPOSITORY"))]),
            context_page: ContextPage::Settings,
            config,
            config_handler,
            config_error,
            accounts: Accounts::new(),
            mailbox: Mailbox::new(),
            reader: Reader::new(),
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
        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            Element::from(menu::root(fl!("view"))),
            menu::items(
                &HashMap::new(),
                vec![
                    menu::Item::Button(fl!("settings"), None, ContextPage::Settings),
                    menu::Item::Button(fl!("about"), None, ContextPage::About),
                ],
            ),
        )]);
        vec![menu_bar.into()]
    }

    fn header_center(&self) -> Vec<Element<'_, Message>> {
        vec![self.search.view(self.narrow).map(Message::Search)]
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        vec![self.composer.new_message_button().map(Message::Composer)]
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.core.window.show_context {
            return None;
        }
        Some(match self.context_page {
            ContextPage::Settings => {
                context_drawer::context_drawer(self.settings(), Message::CloseContext)
                    .title(fl!("settings"))
            }
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_owned()),
                Message::CloseContext,
            )
            .title(fl!("about")),
        })
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
            self.watch_config(Self::APP_ID)
                .map(|update| Message::ConfigUpdated(update.config)),
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
            Message::ToggleContextPage(page) => {
                let show = self.context_page != page || !self.core.window.show_context;
                self.context_page = page;
                self.set_show_context(show);
            }
            Message::CloseContext => self.set_show_context(false),
            Message::LaunchUrl(url) => {
                let _ = open::that_detached(url);
            }
            Message::SetLoadRemoteImages(_) | Message::SetMessageActions(_) => {
                let Some(handler) = &self.config_handler else {
                    return Task::none();
                };
                let mut config = self.config.clone();
                let result = match message {
                    Message::SetLoadRemoteImages(enabled) => {
                        config.set_load_remote_images(handler, enabled)
                    }
                    Message::SetMessageActions(index) => {
                        let Some(&style) = MessageActions::ALL.get(index) else {
                            return Task::none();
                        };
                        config.set_message_actions(handler, style)
                    }
                    _ => unreachable!(),
                };
                self.config_error = result.is_err();
                if !self.config_error {
                    self.apply_config(config);
                }
            }
            Message::ConfigUpdated(config) => self.apply_config(config),
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
                .view(
                    self.mailbox.selected(),
                    self.mailbox.folder(),
                    self.narrow,
                    self.config.message_actions,
                )
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
        if self.composer.is_open() {
            self.composer.update(composer::Message::Save);
            return Task::none();
        }
        if self.core.window.show_context {
            self.set_show_context(false);
            return Task::none();
        }
        self.update_reader(reader::Message::BackToList)
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        self.composer
            .dialog()
            .map(|dialog| dialog.map(Message::Composer))
    }
}

impl AppModel {
    fn settings(&self) -> Element<'_, Message> {
        let mut preferences = widget::settings::section().title(fl!("mail-display")).add(
            widget::settings::item::builder(fl!("mail-show-remote-images")).control(
                widget::toggler(self.config.load_remote_images).on_toggle_maybe(
                    self.config_handler
                        .as_ref()
                        .map(|_| Message::SetLoadRemoteImages),
                ),
            ),
        );
        let selected = MessageActions::ALL
            .iter()
            .position(|&style| style == self.config.message_actions);
        preferences = preferences.add(
            widget::settings::item::builder(fl!("message-actions")).control(widget::dropdown(
                MessageActions::ALL.map(MessageActions::label).to_vec(),
                selected,
                Message::SetMessageActions,
            )),
        );
        if self.config_error {
            preferences = preferences.add(widget::text::body(fl!("settings-error")));
        }

        widget::column::with_capacity(2)
            .spacing(cosmic::theme::spacing().space_m)
            .push(preferences)
            .push(self.accounts.view().map(Message::Accounts))
            .into()
    }

    fn apply_config(&mut self, config: Config) {
        if self.config.load_remote_images != config.load_remote_images {
            self.reader.set_remote_images(config.load_remote_images);
        }
        self.config = config;
    }

    fn update_search(&mut self, message: search::Message) -> Task<Message> {
        match message {
            search::Message::Changed(query) => {
                self.search.set_query(query);
                self.refresh_list()
            }
            search::Message::Focus => self
                .search
                .focus()
                .map(|message| Message::Search(message).into()),
            search::Message::Submit => {
                self.open_selected();
                Task::none()
            }
        }
    }

    fn update_list(&mut self, message: mailbox::Message) -> Task<Message> {
        match message {
            mailbox::Message::Select(id) => {
                let task = self.select(Some(id));
                self.open_selected();
                return task;
            }
            mailbox::Message::Step(direction) => {
                return self.select(self.mailbox.step_selection(self.search.query(), direction));
            }
            mailbox::Message::OpenSelected => self.open_selected(),
            mailbox::Message::Scrolled(offset) => self.mailbox.remember_scroll(offset),
            mailbox::Message::ResumeDraft => self.composer.update(composer::Message::Open),
            mailbox::Message::Refresh => return self.reload_mail(),
            mailbox::Message::Sort(order) => {
                return self
                    .mailbox
                    .set_sort_order(order)
                    .map(|message| Message::List(message).into());
            }
            mailbox::Message::AddAccount => {
                self.context_page = ContextPage::Settings;
                self.set_show_context(true);
                return self
                    .accounts
                    .update(accounts::Message::Add)
                    .map(|message| Message::Accounts(message).into());
            }
            mailbox::Message::Download(request, event) => {
                self.mailbox.update_download(request, event)
            }
            mailbox::Message::Cached(request, contents) => {
                if self.mailbox.finish_load(request, contents, false).is_some() {
                    return self.select(self.mailbox.selection_for(self.search.query()));
                }
            }
            mailbox::Message::Loaded(request, contents) => {
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

    fn update_reader(&mut self, message: reader::Message) -> Task<Message> {
        match message {
            reader::Message::Wake => {}
            reader::Message::BackToList => {
                self.reader.set_open(false);
                return self
                    .mailbox
                    .restore_list_position(self.search.query())
                    .map(|message| Message::List(message).into());
            }
            reader::Message::ShowRemoteImages => self.reader.set_remote_images(true),
            reader::Message::Retry => return self.load_body(),
            reader::Message::Loaded(request, result) => {
                self.reader
                    .finish_load(request, result, self.config.load_remote_images);
            }
            reader::Message::Reply => self.composer.reply(self.mailbox.selected()),
            reader::Message::Forward => {
                if let Some(mail) = self.mailbox.selected()
                    && !self.composer.forward(mail, self.reader.forward_text())
                {
                    self.reader.set_action_error(fl!("forward-draft-exists"));
                }
            }
            reader::Message::ToggleMore => self.reader.toggle_more(),
            reader::Message::Change(change) => {
                let Some(mail) = self.mailbox.selected() else {
                    return Task::none();
                };
                let Some(account) = self.accounts.saved().ok().and_then(|accounts| {
                    accounts
                        .iter()
                        .find(|account| account.id == mail.id.account)
                }) else {
                    self.reader.set_action_error(fl!("mail-unavailable"));
                    return Task::none();
                };
                let Some(request) = self.reader.begin_change() else {
                    return Task::none();
                };
                let folder = self.mailbox.folder();
                return Task::perform(
                    mailbox::remote::change(account.clone(), mail.id.clone(), change),
                    move |result| {
                        Message::Reader(reader::Message::Changed(folder, request, result)).into()
                    },
                );
            }
            reader::Message::Changed(folder, request, result) => {
                self.reader.finish_change(request, result);
                if result.is_ok() && folder == self.mailbox.folder() {
                    return self
                        .mailbox
                        .refresh_after_change(&self.accounts)
                        .map(|message| Message::List(message).into());
                }
            }
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
            Some(Message::ToggleContextPage(ContextPage::Settings))
        }
        keyboard::Key::Character("f") if modifiers.control() => {
            Some(Message::Search(search::Message::Focus))
        }
        keyboard::Key::Named(Named::ArrowUp) => Some(Message::List(mailbox::Message::Step(-1))),
        keyboard::Key::Named(Named::ArrowDown) => Some(Message::List(mailbox::Message::Step(1))),
        keyboard::Key::Named(Named::Enter) => Some(Message::List(mailbox::Message::OpenSelected)),
        _ => None,
    }
}
