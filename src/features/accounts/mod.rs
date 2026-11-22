// SPDX-License-Identifier: GPL-3.0-only

mod connection;
mod model;
mod store;
mod view;

use crate::fl;
use cosmic::Task;
use uuid::Uuid;

pub(super) use connection::{with_imap, with_imap_session};
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
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    Add,
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
            screen: Screen::List,
            status: None,
        }
    }

    pub fn saved(&self) -> Result<&[Account], Error> {
        self.store.as_ref().map_err(|error| *error)?;
        Ok(&self.accounts)
    }

    fn is_busy(&self) -> bool {
        matches!(self.status, Some(Status::Busy))
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if let Message::Finished(result) = message {
            self.status = Some(match result {
                Ok(Outcome::Checked) => Status::Checked,
                Ok(Outcome::Saved(accounts)) => {
                    self.accounts = accounts;
                    self.screen = Screen::List;
                    Status::Saved
                }
                Ok(Outcome::Removed(accounts)) => {
                    self.accounts = accounts;
                    self.screen = Screen::List;
                    Status::Removed
                }
                Err(error) => Status::Error(error),
            });
            return Task::none();
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
            Message::Add => self.screen = Screen::Edit(Form::default()),
            Message::Edit(id) => {
                if let Some(account) = self.accounts.iter().find(|account| account.id == id) {
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
            Message::Finished(_) => unreachable!(),
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
        let editing = form.id.is_some();
        if save {
            let store = match &self.store {
                Ok(store) => store.clone(),
                Err(error) => return self.show_error(*error),
            };
            self.status = Some(Status::Busy);
            Task::perform(
                async move { store.save(account, editing, password).await },
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
