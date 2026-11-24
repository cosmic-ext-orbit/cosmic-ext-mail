// SPDX-License-Identifier: GPL-3.0-only

mod connection;
mod model;
mod online;
mod store;
mod view;

use crate::fl;
use cosmic::Task;
use uuid::Uuid;

pub(super) use connection::{authentication, smtp_transport, with_imap, with_imap_session};
use model::Form;
pub use model::{Account, Field, Password};
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
    OnlineUpgradeRequired,
    OnlineDisabled,
    OnlineDenied,
    OnlineReconnect,
    OnlineConsentTimeout,
    OnlineAlreadyLinked,
    OpenAccountSettings,
}

impl From<accounts::AccountsError> for Error {
    fn from(error: accounts::AccountsError) -> Self {
        use accounts::AccountsError as Broker;
        match error {
            Broker::UpgradeRequired => Self::OnlineUpgradeRequired,
            Broker::Disabled | Broker::Unsupported => Self::OnlineDisabled,
            Broker::NeedsReauth => Self::OnlineReconnect,
            Broker::AccessDenied => Self::OnlineDenied,
            Broker::ConsentTimeout => Self::OnlineConsentTimeout,
            Broker::Failed => Self::OnlineAccessFailed,
            Broker::Unavailable => Self::OnlineUnavailable,
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
            Self::OnlineUpgradeRequired => fl!("account-error-online-upgrade"),
            Self::OnlineDisabled => fl!("account-error-online-disabled"),
            Self::OnlineDenied => fl!("account-error-online-denied"),
            Self::OnlineReconnect => fl!("account-error-online-reconnect"),
            Self::OnlineConsentTimeout => fl!("account-error-online-consent-timeout"),
            Self::OnlineAlreadyLinked => fl!("account-error-online-already-linked"),
            Self::OpenAccountSettings => fl!("account-error-open-settings"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Add,
    Discover,
    Discovered(Result<Vec<Account>, Error>),
    Link(Uuid),
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
    OpenedAccountSettings,
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
    available: Vec<Account>,
    discovering: bool,
    discovery_error: Option<Error>,
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
            accounts,
            available: Vec::new(),
            discovering: false,
            discovery_error: None,
            screen: Screen::List,
            status: None,
        }
    }

    pub fn saved(&self) -> Result<&[Account], Error> {
        self.store.as_ref().map_err(|error| *error)?;
        Ok(&self.accounts)
    }

    pub fn is_editing(&self) -> bool {
        matches!(self.screen, Screen::Edit(_))
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
                return Task::perform(online::available(), Message::Discovered);
            }
            Message::Discovered(result) => {
                self.discovering = false;
                match result {
                    Ok(accounts) => self.available = accounts,
                    Err(error) => {
                        self.available.clear();
                        self.discovery_error = Some(error);
                    }
                }
                return Task::none();
            }
            Message::Finished(result) => {
                self.status = match result {
                    Ok(Outcome::OpenedAccountSettings) => None,
                    Ok(Outcome::Checked) => Some(Status::Checked),
                    Ok(Outcome::Saved(accounts)) => {
                        self.accounts = accounts;
                        self.screen = Screen::List;
                        Some(Status::Saved)
                    }
                    Ok(Outcome::Removed(accounts)) => {
                        self.accounts = accounts;
                        self.screen = Screen::List;
                        Some(Status::Removed)
                    }
                    Err(error) => Some(Status::Error(error)),
                };
                return Task::none();
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
            Message::Add => {
                self.screen = Screen::List;
                self.status = Some(Status::Busy);
                return Task::perform(
                    async {
                        accounts::ui::open_add_account()
                            .map(|()| Outcome::OpenedAccountSettings)
                            .map_err(|_| Error::OpenAccountSettings)
                    },
                    Message::Finished,
                );
            }
            Message::Link(id) => {
                if self.accounts.iter().any(|account| account.id == id) {
                    return self.show_error(Error::OnlineAlreadyLinked);
                }
                let Some(account) = self
                    .available
                    .iter()
                    .find(|account| account.id == id)
                    .cloned()
                else {
                    return Task::none();
                };
                let store = match &self.store {
                    Ok(store) => store.clone(),
                    Err(error) => return self.show_error(*error),
                };
                self.status = Some(Status::Busy);
                return Task::perform(
                    async move {
                        online::mail_access(id, false).await?;
                        store.save(account, false, Password::default()).await
                    },
                    |result| Message::Finished(result.map(Outcome::Saved)),
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
            Message::ConfirmRemove(id) => self.screen = Screen::ConfirmRemove(id),
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
            Message::Discover | Message::Discovered(_) | Message::Finished(_) => unreachable!(),
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
