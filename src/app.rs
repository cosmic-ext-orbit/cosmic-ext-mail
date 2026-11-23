// SPDX-License-Identifier: GPL-3.0-only

use crate::config::{Config, MessageActions};
use crate::features::{
    accounts::{self, Accounts},
    avatars::Avatars,
    composer::{self, Composer},
    mailbox::{self, Mailbox},
    reader::{self, Reader},
    search::{self, Search},
};
use crate::fl;
use cosmic::Application;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuAction {
    Open(ContextPage),
    Quit,
}

impl menu::Action for MenuAction {
    type Message = Message;

    fn message(&self) -> Message {
        match self {
            Self::Open(page) => Message::ToggleContextPage(*page),
            Self::Quit => Message::ExitRequested,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Accounts(accounts::Message),
    Search(search::Message),
    List(mailbox::Message),
    Reader(reader::Message),
    MailChanged(
        mailbox::Folder,
        mailbox::MailId,
        mailbox::remote::Change,
        Result<(), mailbox::remote::Error>,
    ),
    CloseToast(widget::ToastId),
    Composer(composer::Message),
    ResizePanes(pane_grid::ResizeEvent),
    ToggleContextPage(ContextPage),
    CloseContext,
    ExitRequested,
    CloseWindow,
    WindowOpened,
    WindowClosed(window::Id),
    CheckInbox,
    InboxChecked(bool),
    WindowCloseRequested(window::Id),
    LaunchUrl(String),
    SetLoadRemoteImages(bool),
    SetShowSenderPictures(bool),
    SetFetchSenderPhotos(bool),
    SenderPhoto(String, Option<cosmic::iced::widget::image::Handle>),
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
    avatars: Avatars,
    reader: Reader,
    toasts: widget::Toasts<Message>,
    search: Search,
    composer: Composer,
    panes: pane_grid::State<Pane>,
    narrow: bool,
    compact_sidebar: bool,
    checking_inbox: bool,
    inbox_check_failed: bool,
    inbox_retry_at: tokio::time::Instant,
}

pub struct Flags;

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = String;
    type Args = Vec<String>;
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;

    const APP_ID: &'static str = "dev.edfloreshz.Mail";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _: Flags) -> (Self, Task<Message>) {
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
            avatars: Avatars::default(),
            reader: Reader::new(),
            toasts: widget::Toasts::new(Message::CloseToast),
            search: Search::new(),
            composer: Composer::default(),
            panes,
            narrow: false,
            compact_sidebar: false,
            checking_inbox: false,
            inbox_check_failed: false,
            inbox_retry_at: tokio::time::Instant::now(),
        };
        let open = app.open_window();
        let load = app.reload_mail();
        (app, Task::batch([open, load]))
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        let menu_bar = menu::bar(vec![menu::Tree::with_children(
            Element::from(menu::root(fl!("view"))),
            menu::items(
                &HashMap::new(),
                vec![
                    menu::Item::Button(
                        fl!("settings"),
                        None,
                        MenuAction::Open(ContextPage::Settings),
                    ),
                    menu::Item::Button(fl!("about"), None, MenuAction::Open(ContextPage::About)),
                    menu::Item::Button(fl!("mail-quit"), None, MenuAction::Quit),
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
        let photos = self
            .sender_photos()
            .map_or_else(Subscription::none, |avatars| {
                avatars
                    .subscription(self.mailbox.visible_sender_addresses(self.search.query()))
                    .map(|(address, photo)| Message::SenderPhoto(address, photo))
            });
        Subscription::batch([
            photos,
            self.composer
                .subscription(self.core.main_window_id())
                .map(Message::Composer),
            cosmic::iced::time::every(std::time::Duration::from_secs(60))
                .map(|_| Message::CheckInbox),
            cosmic::iced::window::close_requests().map(Message::WindowCloseRequested),
            self.reader.subscription().map(Message::Reader),
            event::listen_with(shortcut)
                .with((self.core.main_window_id(), self.composer.is_open()))
                .filter_map(|((main_window, composing), (id, message))| {
                    (!composing && Some(id) == main_window).then_some(message)
                }),
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
            Message::CloseToast(id) => self.toasts.remove(id),
            Message::MailChanged(source, id, change, result) => {
                let restored = self.mailbox.finish_change(&id, result.is_ok());
                if let Err(error) = result {
                    let toast = self.mail_action_error(error.text());
                    if restored {
                        self.reader.set_open(true);
                        return Task::batch([toast, self.load_body()]);
                    }
                    return toast;
                }
                let current = self.mailbox.folder();
                if current == source
                    || matches!(change, mailbox::remote::Change::Move(destination) if current == destination)
                {
                    return self
                        .mailbox
                        .refresh_after_change(&self.accounts)
                        .map(|message| Message::List(message).into());
                }
            }
            Message::Composer(message) => {
                return self
                    .composer
                    .update(message, self.accounts.saved().unwrap_or_default())
                    .map(|message| Message::Composer(message).into());
            }
            Message::ResizePanes(event) => self.panes.resize(event.split, event.ratio),
            Message::ToggleContextPage(page) => {
                let show = self.context_page != page || !self.core.window.show_context;
                self.context_page = page;
                self.set_show_context(show);
            }
            Message::CloseContext => self.set_show_context(false),
            Message::WindowCloseRequested(id) => {
                if self.core.main_window_is(id) {
                    return self.update(Message::CloseWindow);
                }
            }
            Message::CloseWindow => {
                if self.composer.prepare_to_exit()
                    && let Some(id) = self.core.main_window_id()
                {
                    return window::close(id);
                }
            }
            Message::WindowClosed(id) => {
                if self.core.main_window_is(id) {
                    self.core.set_main_window_id(None);
                    self.reader.release_window();
                    self.mailbox.cancel_sync();
                    return self.check_inbox();
                }
            }
            Message::WindowOpened => {
                let Some(id) = self.core.main_window_id() else {
                    return Task::none();
                };
                let title = self.set_window_title(fl!("app-title"), id);
                let refresh = self
                    .mailbox
                    .refresh(&self.accounts)
                    .map(|message| Message::List(message).into());
                return Task::batch([title, refresh, self.load_body()]);
            }
            Message::CheckInbox => return self.check_inbox(),
            Message::InboxChecked(success) => {
                self.checking_inbox = false;
                self.inbox_check_failed = !success;
                if !success {
                    self.inbox_retry_at =
                        tokio::time::Instant::now() + std::time::Duration::from_secs(120);
                }
            }
            Message::ExitRequested => {
                return if self.composer.prepare_to_exit() {
                    self.mailbox.cancel_sync();
                    cosmic::iced::exit()
                } else {
                    Task::none()
                };
            }
            Message::LaunchUrl(url) => {
                let _ = open::that_detached(url);
            }
            Message::SetLoadRemoteImages(_)
            | Message::SetShowSenderPictures(_)
            | Message::SetFetchSenderPhotos(_)
            | Message::SetMessageActions(_) => {
                let Some(handler) = &self.config_handler else {
                    return Task::none();
                };
                let mut config = self.config.clone();
                let result = match message {
                    Message::SetLoadRemoteImages(enabled) => {
                        config.set_load_remote_images(handler, enabled)
                    }
                    Message::SetShowSenderPictures(enabled) => {
                        config.set_show_recipient_pictures(handler, enabled)
                    }
                    Message::SetFetchSenderPhotos(enabled) => {
                        config.set_fetch_sender_photos(handler, enabled)
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
            Message::SenderPhoto(address, photo) => {
                if self.sender_photos().is_some() {
                    self.avatars.save(address, photo);
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let list = || {
            self.mailbox
                .view(
                    self.search.query(),
                    &self.composer.draft_rows(),
                    self.composer.draft_read_error(),
                    self.config.show_recipient_pictures,
                    self.sender_photos(),
                )
                .map(Message::List)
        };
        let reader = || {
            self.reader
                .view(
                    self.mailbox.selected(),
                    self.mailbox.folder(),
                    self.narrow,
                    self.mailbox.selected_change_pending(),
                    &self.config,
                    self.sender_photos(),
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

        widget::toaster(&self.toasts, content).into()
    }

    fn footer(&self) -> Option<Element<'_, Message>> {
        use mailbox::{Activity, FetchActivity};

        let activity = self.mailbox.activity();
        let label = match activity {
            Activity::Changing => Some(fl!("mail-change-working")),
            _ if self.composer.is_sending() => Some(fl!("message-sending")),
            Activity::NoAccounts => Some(fl!("mail-status-no-accounts")),
            Activity::SettingsError => Some(fl!("account-error-settings")),
            Activity::SyncFailed => Some(fl!("mail-status-sync-failed")),
            Activity::Idle if self.inbox_check_failed => Some(fl!("mail-status-inbox-failed")),
            Activity::Idle => None,
        };
        let status: Element<_> = match label {
            Some(label) => widget::text::caption(label).width(Length::Fill).into(),
            None => widget::space::horizontal().width(Length::Fill).into(),
        };

        let fetching = match self.mailbox.fetch_activity() {
            Some(FetchActivity::Downloading { saved, total }) => Some((
                fl!("mail-download-progress", saved = saved, total = total),
                widget::determinate_linear(saved as f32 / total as f32),
            )),
            Some(FetchActivity::Checking) => {
                Some((fl!("mail-refresh-syncing"), widget::indeterminate_linear()))
            }
            None if self.checking_inbox => {
                Some((fl!("mail-checking-inbox"), widget::indeterminate_linear()))
            }
            None if self.reader.is_loading() => {
                Some((fl!("mail-body-loading"), widget::indeterminate_linear()))
            }
            None => None,
        };

        let fetch_status = fetching
            .map(|(label, progress)| (label, Some(progress)))
            .or_else(|| {
                (matches!(activity, Activity::Changing | Activity::Idle)
                    && !self.inbox_check_failed)
                    .then(|| (fl!("mail-status-idle"), None))
            })
            .map(|(label, progress)| {
                widget::row::with_capacity(2)
                    .push(
                        widget::text::caption(label)
                            .align_x(cosmic::iced::alignment::Horizontal::Right)
                            .width(Length::Fill),
                    )
                    .push_maybe(progress.map(|bar| bar.width(Length::Fixed(80.0))))
                    .align_y(cosmic::iced::Alignment::Center)
                    .spacing(12)
                    .width(Length::Fill)
            });

        Some(
            widget::container(
                widget::row::with_capacity(2)
                    .push(status)
                    .push_maybe(fetch_status)
                    .align_y(cosmic::iced::Alignment::Center)
                    .spacing(12),
            )
            .padding([0, mailbox::CONTENT_GUTTER])
            .into(),
        )
    }

    fn on_app_exit(&mut self) -> Option<Message> {
        Some(Message::CloseWindow)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        self.core
            .main_window_is(id)
            .then_some(Message::WindowClosed(id))
    }

    fn dbus_activation(&mut self, message: cosmic::dbus_activation::Message) -> Task<Message> {
        if let cosmic::dbus_activation::Details::ActivateAction { action, .. } = message.msg
            && action == "quit"
        {
            return self.update(Message::ExitRequested);
        }
        self.open_window()
    }

    fn on_escape(&mut self) -> Task<Message> {
        if self.composer.is_open() {
            return self.update(Message::Composer(composer::Message::Close));
        }
        if self.core.window.show_context {
            self.set_show_context(false);
            return Task::none();
        }
        self.update_reader(reader::Message::BackToList)
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        self.composer
            .dialog(self.accounts.saved().unwrap_or_default())
            .map(|dialog| dialog.map(Message::Composer))
    }
}

impl AppModel {
    fn sender_photos(&self) -> Option<&Avatars> {
        (self.core.main_window_id().is_some()
            && self.config.show_recipient_pictures
            && self.config.fetch_sender_photos)
            .then_some(&self.avatars)
    }

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
        preferences = preferences.add(
            widget::settings::item::builder(fl!("show-sender-pictures")).control(
                widget::toggler(self.config.show_recipient_pictures).on_toggle_maybe(
                    self.config_handler
                        .as_ref()
                        .map(|_| Message::SetShowSenderPictures),
                ),
            ),
        );
        preferences = preferences
            .add(
                widget::settings::item::builder(fl!("fetch-sender-photos")).control(
                    widget::toggler(self.config.fetch_sender_photos).on_toggle_maybe(
                        self.config_handler
                            .as_ref()
                            .map(|_| Message::SetFetchSenderPhotos),
                    ),
                ),
            )
            .add(widget::text::caption(fl!(
                "fetch-sender-photos-description"
            )));
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
            search::Message::Submit => self.open_selected(),
        }
    }

    fn update_list(&mut self, message: mailbox::Message) -> Task<Message> {
        match message {
            mailbox::Message::Select(id) => {
                let task = self.select(Some(id));
                return Task::batch([task, self.open_selected()]);
            }
            mailbox::Message::Step(direction) => {
                let task = self.select(self.mailbox.step_selection(self.search.query(), direction));
                return if self.narrow {
                    task
                } else {
                    Task::batch([task, self.open_selected()])
                };
            }
            mailbox::Message::OpenSelected => return self.open_selected(),
            mailbox::Message::Scrolled(offset) => self.mailbox.remember_scroll(offset),
            mailbox::Message::ResumeDraft(id) => {
                return self.update(Message::Composer(composer::Message::OpenDraft(id)));
            }
            mailbox::Message::Refresh => {
                return self
                    .mailbox
                    .refresh(&self.accounts)
                    .map(|message| Message::List(message).into());
            }
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
                if self.mailbox.is_current_request(request)
                    && self.mailbox.folder() == mailbox::Folder::Inbox
                {
                    self.inbox_check_failed = !contents.failures.is_empty();
                    if self.inbox_check_failed {
                        self.inbox_retry_at =
                            tokio::time::Instant::now() + std::time::Duration::from_secs(120);
                    }
                }
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
            reader::Message::Reply => {
                if let Some(mail) = self.mailbox.selected() {
                    return self
                        .composer
                        .reply(
                            mail,
                            self.reader.reply_address(mail),
                            self.reader.message_id(),
                            self.reader.forward_text(),
                        )
                        .map(|message| Message::Composer(message).into());
                }
            }
            reader::Message::Forward => {
                if let Some(mail) = self.mailbox.selected() {
                    return self
                        .composer
                        .forward(mail, self.reader.forward_text())
                        .map(|message| Message::Composer(message).into());
                }
            }
            reader::Message::CopyAddress(address) => {
                return cosmic::iced::clipboard::write(address);
            }
            reader::Message::ComposeTo(address) => {
                let account = self.mailbox.selected().map(|mail| mail.id.account);
                return self
                    .composer
                    .compose_to(address, account)
                    .map(|message| Message::Composer(message).into());
            }
            reader::Message::Change(change) => return self.change_selected(change),
        }
        Task::none()
    }

    fn open_window(&mut self) -> Task<Message> {
        if let Some(id) = self.core.main_window_id() {
            return window::gain_focus(id);
        }
        let settings = window::Settings {
            size: cosmic::iced::Size::new(1400.0, 900.0),
            min_size: Some(cosmic::iced::Size::new(360.0, 180.0)),
            decorations: false,
            transparent: true,
            exit_on_close_request: false,
            platform_specific: window::settings::PlatformSpecific {
                application_id: Self::APP_ID.to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        let (id, open) = window::open(settings);
        self.core.set_main_window_id(Some(id));
        open.map(|_| Message::WindowOpened.into())
    }

    fn check_inbox(&mut self) -> Task<Message> {
        if self.checking_inbox || tokio::time::Instant::now() < self.inbox_retry_at {
            return Task::none();
        }
        if self.mailbox.folder() == mailbox::Folder::Inbox {
            return self
                .mailbox
                .poll_inbox(&self.accounts)
                .map(|message| Message::List(message).into());
        }
        let Ok(accounts) = self.accounts.saved() else {
            return Task::none();
        };
        if accounts.is_empty() {
            return Task::none();
        }
        self.checking_inbox = true;
        Task::perform(mailbox::remote::poll_inbox(accounts.to_vec()), |success| {
            Message::InboxChecked(success).into()
        })
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
        if self.core.main_window_id().is_none() {
            return Task::none();
        }
        self.reader
            .load(self.mailbox.selected(), &self.accounts)
            .map(|message| Message::Reader(message).into())
    }

    fn open_selected(&mut self) -> Task<Message> {
        let Some(mail) = self.mailbox.selected() else {
            return Task::none();
        };
        self.reader.set_open(true);
        if mail.unread {
            self.change_selected(mailbox::remote::Change::Seen(true))
        } else {
            Task::none()
        }
    }

    fn change_selected(&mut self, change: mailbox::remote::Change) -> Task<Message> {
        let Some(mail) = self.mailbox.selected() else {
            return Task::none();
        };
        let account = self
            .accounts
            .saved()
            .ok()
            .and_then(|accounts| {
                accounts
                    .iter()
                    .find(|account| account.id == mail.id.account)
            })
            .cloned();
        let Some(account) = account else {
            return self.mail_action_error(fl!("mail-unavailable"));
        };
        let Some((source, id)) = self.mailbox.begin_change(change) else {
            return Task::none();
        };
        let request = Task::perform(
            mailbox::remote::change(account, source, id.clone(), change),
            move |result| Message::MailChanged(source, id, change, result).into(),
        );
        if matches!(change, mailbox::remote::Change::Move(_)) {
            self.reader.set_open(false);
            return Task::batch([request, self.load_body()]);
        }
        request
    }

    fn mail_action_error(&mut self, error: String) -> Task<Message> {
        self.toasts
            .push(widget::Toast::new(error))
            .map(cosmic::action::app)
    }
}

fn shortcut(event: Event, status: event::Status, id: window::Id) -> Option<(window::Id, Message)> {
    use keyboard::key::Named;

    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    if status == event::Status::Captured {
        return None;
    }
    let message = match key.as_ref() {
        keyboard::Key::Character(",") if modifiers.control() => {
            Some(Message::ToggleContextPage(ContextPage::Settings))
        }
        keyboard::Key::Character("q") if modifiers.control() => Some(Message::ExitRequested),
        keyboard::Key::Character("f") if modifiers.control() => {
            Some(Message::Search(search::Message::Focus))
        }
        keyboard::Key::Named(Named::ArrowUp) => Some(Message::List(mailbox::Message::Step(-1))),
        keyboard::Key::Named(Named::ArrowDown) => Some(Message::List(mailbox::Message::Step(1))),
        keyboard::Key::Named(Named::Enter) => Some(Message::List(mailbox::Message::OpenSelected)),
        _ => None,
    }?;
    Some((id, message))
}
