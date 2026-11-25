// SPDX-License-Identifier: GPL-3.0-only

mod connection;
mod model;
mod online;
mod store;
mod view;

use accounts::{AuthRequest, Provider};
use crate::fl;
use cosmic::Task;
use std::collections::HashSet;
use uuid::Uuid;

pub(super) use connection::{authentication, smtp_transport, with_imap, with_imap_session};
use model::Form;
pub use model::{Account, Field, Password};
pub(crate) use online::{changes, enabled_ids, graph_token};
use store::Store;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Email,
    Username,
    Password,
    ImapSettings,
    SmtpSettings,
    Keyring,
    KeyringLocked,
    PasswordMissing,
    Settings,
    StorageRollback,
    StorageBusy,
    AccountMissing,
    ImapConnect,
    ImapLogin,
    SmtpConnect,
    ConnectionTask,
    ConnectionTimeout,
    OnlineUnavailable,
    OnlineAccessFailed,
    OnlineDisabled,
    OnlineReconnect,
    OnlineConsentTimeout,
    OnlineAuthFailed,
    OnlineCancelled,
    OnlineAlreadyLinked,
}

impl From<accounts::Error> for Error {
    fn from(error: accounts::Error) -> Self {
        use accounts::Error as Library;
        match error {
            Library::StorageUnavailable(_) | Library::StorageCorrupt(_) => {
                Self::OnlineUnavailable
            }
            Library::InvalidClientConfig(_) | Library::MissingClientConfig(_, _) => {
                Self::OnlineAccessFailed
            }
            Library::AuthenticationFailed(_) | Library::InvalidState => Self::OnlineAuthFailed,
            Library::Cancelled => Self::OnlineCancelled,
            Library::CallbackTimeout => Self::OnlineConsentTimeout,
            Library::NeedsReauth(_) => Self::OnlineReconnect,
            Library::FeatureDisabled(_) | Library::UnsupportedProvider(_, _) => {
                Self::OnlineDisabled
            }
            Library::AccountAlreadyExists(_, _) => Self::OnlineAlreadyLinked,
            Library::AccountNotFound(_)
            | Library::InvalidEndpoint(_)
            | Library::Network(_)
            | Library::ProviderResponse(_) => Self::OnlineAccessFailed,
        }
    }
}

impl Error {
    pub(super) fn text(self) -> String {
        match self {
            Self::Email => fl!("account-error-email"),
            Self::Username => fl!("account-error-username"),
            Self::Password => fl!("account-error-password"),
            Self::ImapSettings => fl!("account-error-imap-settings"),
            Self::SmtpSettings => fl!("account-error-smtp-settings"),
            Self::Keyring => fl!("account-error-keyring"),
            Self::KeyringLocked => fl!("account-error-keyring-locked"),
            Self::PasswordMissing => fl!("account-error-password-missing"),
            Self::Settings => fl!("account-error-settings"),
            Self::StorageRollback => fl!("account-error-storage-rollback"),
            Self::StorageBusy => fl!("account-error-storage-busy"),
            Self::AccountMissing => fl!("account-error-missing"),
            Self::ImapConnect => fl!("account-error-imap-connect"),
            Self::ImapLogin => fl!("account-error-imap-login"),
            Self::SmtpConnect => fl!("account-error-smtp-connect"),
            Self::ConnectionTask => fl!("account-error-connection-task"),
            Self::ConnectionTimeout => fl!("account-error-connection-timeout"),
            Self::OnlineUnavailable => fl!("account-error-online-unavailable"),
            Self::OnlineAccessFailed => fl!("account-error-online-access-failed"),
            Self::OnlineDisabled => fl!("account-error-online-disabled"),
            Self::OnlineReconnect => fl!("account-error-online-reconnect"),
            Self::OnlineConsentTimeout => fl!("account-error-online-consent-timeout"),
            Self::OnlineAuthFailed => fl!("account-error-online-auth-failed"),
            Self::OnlineCancelled => fl!("account-error-online-cancelled"),
            Self::OnlineAlreadyLinked => fl!("account-error-online-already-linked"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Add,
    AddLocal,
    AddProvider(Provider),
    AuthStarted(Result<AuthRequest, Error>),
    AuthCompleted(Result<accounts::Account, Error>),
    CancelAuth(String),
    Reconnect(Uuid),
    RemoveOnline(Uuid),
    RemovedOnline(Result<(), Error>),
    ToggleFinished(Result<(Uuid, bool), Error>),
    Discover,
    Discovered(Result<Vec<accounts::Account>, Error>),
    ToggleLink(Uuid, bool),
    Edit(Uuid),
    Change(Field, String),
    PasswordChanged(Password),
    StartTlsChanged(bool),
    Check,
    Save,
    ConfirmRemove(Uuid),
    Remove(Uuid),
    Back,
    Finished(Result<Outcome, Error>),
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Checked,
    Saved(Vec<Account>),
    Removed(Vec<Account>),
}

enum Screen {
    List,
    Edit(Form),
    ConfirmRemove(Uuid),
}

#[derive(Clone, Copy)]
enum Status {
    Busy,
    Checked,
    Saved,
    Removed,
    Error(Error),
}

impl Status {
    fn text(self) -> String {
        match self {
            Self::Busy => fl!("account-working"),
            Self::Checked => fl!("account-check-success"),
            Self::Saved => fl!("account-save-success"),
            Self::Removed => fl!("account-remove-success"),
            Self::Error(error) => error.text(),
        }
    }
}

pub struct Accounts {
    store: Result<Store, Error>,
    accounts: Vec<Account>,
    active: Vec<Account>,
    managed: Vec<accounts::Account>,
    available: Vec<Account>,
    /// Account IDs whose Mail service is currently enabled.
    ///
    /// `None` means the broker has not been queried yet, so saved online
    /// accounts stay hidden until the first successful check.
    usable_online: Option<HashSet<Uuid>>,
    discovering: bool,
    discovery_error: Option<Error>,
    auth: Option<AuthRequest>,
    screen: Screen,
    status: Option<Status>,
}

impl Accounts {
    pub fn new() -> Self {
        let (store, accounts) = match Store::open() {
            Ok((store, accounts)) => (Ok(store), accounts),
            Err(error) => (Err(error), Vec::new()),
        };
        Self {
            store,
            active: accounts
                .iter()
                .filter(|account| !account.online)
                .cloned()
                .collect(),
            accounts,
            managed: Vec::new(),
            available: Vec::new(),
            usable_online: None,
            discovering: false,
            discovery_error: None,
            auth: None,
            screen: Screen::List,
            status: None,
        }
    }

    /// Accounts Mail may actually use for mail traffic.
    ///
    /// Local accounts are always usable. Embedded online accounts are usable
    /// when their Mail service is enabled.
    pub fn usable(&self) -> Result<&[Account], Error> {
        self.store.as_ref().map_err(|error| *error)?;
        Ok(&self.active)
    }

    pub fn usable_vec(&self) -> Vec<Account> {
        self.usable().unwrap_or_default().to_vec()
    }

    pub fn usable_ids(&self) -> Vec<Uuid> {
        self.usable()
            .unwrap_or_default()
            .iter()
            .map(|account| account.id)
            .collect()
    }

    /// Replaces the set of online accounts known to be usable.
    pub fn set_usable_online(&mut self, ids: Vec<Uuid>) {
        self.usable_online = Some(ids.into_iter().collect());
        self.recompute_active();
    }

    pub fn set_online_usable(&mut self, id: Uuid, enabled: bool) {
        let ids = self.usable_online.get_or_insert_with(HashSet::new);
        if enabled {
            ids.insert(id);
        } else {
            ids.remove(&id);
        }
        self.recompute_active();
    }

    fn set_accounts(&mut self, accounts: Vec<Account>) {
        self.accounts = accounts;
        self.recompute_active();
    }

    fn recompute_active(&mut self) {
        self.active = self
            .accounts
            .iter()
            .filter(|account| {
                !account.online
                    || self
                        .usable_online
                        .as_ref()
                        .is_some_and(|ids| ids.contains(&account.id))
            })
            .cloned()
            .collect();
    }

    pub fn is_editing(&self) -> bool {
        matches!(self.screen, Screen::Edit(_))
    }

    pub(super) fn auth(&self) -> Option<&AuthRequest> {
        self.auth.as_ref()
    }

    fn is_busy(&self) -> bool {
        matches!(self.status, Some(Status::Busy))
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Discover => {
                if self.discovering {
                    return Task::none();
                }
                self.discovering = true;
                self.discovery_error = None;
                return Task::perform(online::managed(), Message::Discovered);
            }
            Message::Discovered(result) => {
                self.discovering = false;
                match result {
                    Ok(managed) => {
                        self.available = online::enabled_from(&managed);
                        self.set_usable_online(
                            self.available.iter().map(|account| account.id).collect(),
                        );
                        self.managed = managed;
                    }
                    Err(error) => {
                        self.available.clear();
                        self.managed.clear();
                        self.discovery_error = Some(error);
                    }
                }
                return Task::none();
            }
            Message::Finished(result) => {
                self.status = match result {
                    Ok(Outcome::Checked) => Some(Status::Checked),
                    Ok(Outcome::Saved(accounts)) => {
                        self.set_accounts(accounts);
                        self.screen = Screen::List;
                        Some(Status::Saved)
                    }
                    Ok(Outcome::Removed(accounts)) => {
                        self.set_accounts(accounts);
                        self.screen = Screen::List;
                        Some(Status::Removed)
                    }
                    Err(error) => Some(Status::Error(error)),
                };
                return Task::none();
            }
            Message::AuthStarted(ref result) => match result {
                Ok(request) => {
                    self.auth = Some(request.clone());
                    self.status = Some(Status::Busy);
                    return Task::perform(online::complete(request.clone()), |result| {
                        Message::AuthCompleted(result)
                    });
                }
                Err(error) => {
                    self.auth = None;
                    self.status = Some(Status::Error(*error));
                }
            },
            Message::AuthCompleted(result) => {
                self.auth = None;
                self.status = match result {
                    Ok(_) => Some(Status::Saved),
                    Err(Error::OnlineCancelled) => None,
                    Err(error) => Some(Status::Error(error)),
                };
                return Task::perform(online::managed(), Message::Discovered);
            }
            Message::CancelAuth(state) => {
                online::cancel(&state);
                self.auth = None;
                self.status = None;
                return Task::perform(online::managed(), Message::Discovered);
            }
            Message::RemovedOnline(result) => {
                self.status = match result {
                    Ok(()) => Some(Status::Removed),
                    Err(error) => Some(Status::Error(error)),
                };
                return Task::perform(online::managed(), Message::Discovered);
            }
            Message::ToggleFinished(result) => {
                match result {
                    Ok((id, enabled)) => {
                        let ids = self.usable_online.get_or_insert_with(HashSet::new);
                        if enabled {
                            ids.insert(id);
                        } else {
                            ids.remove(&id);
                        }
                        self.status = None;
                    }
                    Err(error) => self.status = Some(Status::Error(error)),
                }
                return Task::perform(online::managed(), Message::Discovered);
            }
            _ => {}
        }

        if self.is_busy() {
            return Task::none();
        }

        self.status = None;
        match message {
            Message::Back => {
                if let Ok(store) = &self.store {
                    match store.load() {
                        Ok(accounts) => self.accounts = accounts,
                        Err(error) => self.status = Some(Status::Error(error)),
                    }
                }
                self.screen = Screen::List;
            }
            Message::Add | Message::AddLocal => {
                self.screen = Screen::Edit(Form::new());
            }
            Message::AddProvider(provider) => {
                self.status = Some(Status::Busy);
                return Task::perform(online::begin(provider), Message::AuthStarted);
            }
            Message::Reconnect(id) => {
                self.status = Some(Status::Busy);
                return Task::perform(online::reconnect(id), Message::AuthStarted);
            }
            Message::RemoveOnline(id) => {
                self.status = Some(Status::Busy);
                return Task::perform(online::remove(id), Message::RemovedOnline);
            }
            Message::ToggleLink(id, enabled) => {
                self.set_online_usable(id, enabled);
                self.status = Some(Status::Busy);
                return Task::perform(
                    async move { online::set_enabled(id, enabled).await.map(|()| (id, enabled)) },
                    Message::ToggleFinished,
                );
            }
            Message::Edit(id) => {
                if let Some(account) = self
                    .accounts
                    .iter()
                    .find(|account| account.id == id && !account.online)
                {
                    self.screen = Screen::Edit(Form::edit(account));
                }
            }
            Message::Change(field, value) => {
                if let Screen::Edit(form) = &mut self.screen {
                    form.change(field, value);
                }
            }
            Message::PasswordChanged(password) => {
                if let Screen::Edit(form) = &mut self.screen {
                    form.password = password;
                }
            }
            Message::StartTlsChanged(enabled) => {
                if let Screen::Edit(form) = &mut self.screen {
                    if form.smtp_port == if form.smtp_starttls { "587" } else { "465" } {
                        form.smtp_port = if enabled { "587" } else { "465" }.into();
                    }
                    form.smtp_starttls = enabled;
                }
            }
            Message::Check | Message::Save => return self.submit(matches!(message, Message::Save)),
            Message::ConfirmRemove(id) => {
                if self
                    .accounts
                    .iter()
                    .any(|account| account.id == id && account.online)
                {
                    return Task::none();
                }
                self.screen = Screen::ConfirmRemove(id);
            }
            Message::Remove(id) => {
                if !matches!(self.screen, Screen::ConfirmRemove(current) if current == id) {
                    return Task::none();
                }
                let store = match &self.store {
                    Ok(store) => store.clone(),
                    Err(error) => return self.show_error(*error),
                };
                self.status = Some(Status::Busy);
                return Task::perform(async move { store.remove(id).await }, |result| {
                    Message::Finished(result.map(Outcome::Removed))
                });
            }
            _ => {}
        }
        Task::none()
    }

    fn submit(&mut self, save: bool) -> Task<Message> {
        let Screen::Edit(form) = &self.screen else {
            return Task::none();
        };
        let account = match form.validate() {
            Ok(account) => account,
            Err(error) => return self.show_error(error),
        };
        let password = form.password.clone();
        if save {
            let store = match &self.store {
                Ok(store) => store.clone(),
                Err(error) => return self.show_error(*error),
            };
            self.status = Some(Status::Busy);
            Task::perform(
                async move { store.save(account, true, password).await },
                |result| Message::Finished(result.map(Outcome::Saved)),
            )
        } else {
            self.status = Some(Status::Busy);
            Task::perform(connection::check(account, password), |result| {
                Message::Finished(result.map(|()| Outcome::Checked))
            })
        }
    }

    fn show_error(&mut self, error: Error) -> Task<Message> {
        self.status = Some(Status::Error(error));
        Task::none()
    }
}
