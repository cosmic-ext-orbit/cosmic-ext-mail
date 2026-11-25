// SPDX-License-Identifier: GPL-3.0-only

mod connection;
mod model;
mod online;
mod view;

use accounts::{AuthRequest, Provider};
use cosmic::Task;
use std::collections::HashSet;
use uuid::Uuid;

use crate::fl;
use accounts::ui::FormField;

pub(super) use connection::{authentication, smtp_transport, with_imap, with_imap_session};
use model::MailForm;
pub use model::{Account, Password};
pub(crate) use online::{changes, enabled_ids, graph_token};
pub use view::view;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Email,
    Username,
    Password,
    ImapSettings,
    SmtpSettings,
    AccountMissing,
    ImapConnect,
    ImapLogin,
    SmtpConnect,
    ConnectionTask,
    ConnectionTimeout,
    OnlineUnavailable,
    OnlineNotConfigured,
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
            Library::StorageUnavailable(_) | Library::StorageCorrupt(_) => Self::OnlineUnavailable,
            Library::InvalidClientConfig(_) | Library::MissingClientConfig(_, _) => {
                Self::OnlineNotConfigured
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
            Self::AccountMissing => fl!("account-error-missing"),
            Self::ImapConnect => fl!("account-error-imap-connect"),
            Self::ImapLogin => fl!("account-error-imap-login"),
            Self::SmtpConnect => fl!("account-error-smtp-connect"),
            Self::ConnectionTask => fl!("account-error-connection-task"),
            Self::ConnectionTimeout => fl!("account-error-connection-timeout"),
            Self::OnlineUnavailable => fl!("account-error-online-unavailable"),
            Self::OnlineNotConfigured => fl!("account-error-online-not-configured"),
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
    Discover,
    Discovered(Result<Vec<accounts::Account>, Error>),
    AddOnlineOpen,
    AddOnlineClose,
    AddProvider(Provider),
    AddCustomImapSmtp,
    CustomFieldChanged(FormField, String),
    CustomStartTlsChanged(bool),
    SubmitCustomImapSmtp,
    CustomImapSmtpAdded(Result<accounts::Account, Error>),
    CancelCustomForm,
    OpenBrowser,
    AuthStarted(Result<AuthRequest, Error>),
    AuthCompleted(Result<accounts::Account, Error>),
    CancelAuth(String),
    Reconnect(Uuid),
    RemoveOnline(Uuid),
    RemovedOnline(Result<(), Error>),
    ToggleFinished(Result<(Uuid, bool), Error>),
    ToggleLink(Uuid, bool),
    /// Account alerts are delivered by the host application's toaster.
    Notify(String),
}

fn notify(message: String) -> Task<Message> {
    cosmic::task::message(Message::Notify(message))
}

pub struct Accounts {
    managed: Vec<accounts::Account>,
    available: Vec<Account>,
    /// Account IDs whose Mail service is currently enabled, or `None` before
    /// the embedded store has been queried.
    usable_online: Option<HashSet<Uuid>>,
    discovering: bool,
    auth: Option<AuthRequest>,
    adding_online: bool,
    custom_form: Option<MailForm>,
    busy: bool,
}

impl Accounts {
    pub fn new() -> Self {
        Self {
            managed: Vec::new(),
            available: Vec::new(),
            usable_online: None,
            discovering: false,
            auth: None,
            adding_online: false,
            custom_form: None,
            busy: false,
        }
    }

    /// Accounts Mail may actually use. Every account lives in the app's
    /// embedded, encrypted account store; there is no separate local editor.
    pub fn usable(&self) -> Result<&[Account], Error> {
        Ok(&self.available)
    }

    pub fn usable_vec(&self) -> Vec<Account> {
        self.available.clone()
    }

    pub fn usable_ids(&self) -> Vec<Uuid> {
        self.available.iter().map(|account| account.id).collect()
    }

    /// Replaces the set of enabled account IDs after the manager reports a
    /// change. Saved accounts stay hidden until the first successful check.
    pub fn set_usable_online(&mut self, ids: Vec<Uuid>) {
        self.usable_online = Some(ids.into_iter().collect());
        self.recompute();
    }

    fn set_online_usable(&mut self, id: Uuid, enabled: bool) {
        let ids = self.usable_online.get_or_insert_with(HashSet::new);
        if enabled {
            ids.insert(id);
        } else {
            ids.remove(&id);
        }
        self.recompute();
    }

    fn recompute(&mut self) {
        let mut accounts = online::enabled_from(&self.managed);
        if let Some(ids) = &self.usable_online {
            accounts.retain(|account| ids.contains(&account.id));
        }
        self.available = accounts;
    }

    pub fn is_editing(&self) -> bool {
        self.custom_form.is_some()
    }

    pub fn is_adding_online(&self) -> bool {
        self.adding_online
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Notify(_) => {}
            Message::Discover => {
                if self.discovering {
                    return Task::none();
                }
                self.discovering = true;
                return Task::perform(online::managed(), Message::Discovered);
            }
            Message::Discovered(result) => {
                self.discovering = false;
                match result {
                    Ok(managed) => {
                        self.available = online::enabled_from(&managed);
                        self.usable_online =
                            Some(self.available.iter().map(|account| account.id).collect());
                        self.managed = managed;
                    }
                    Err(error) => {
                        self.available.clear();
                        self.managed.clear();
                        return notify(error.text());
                    }
                }
                return Task::none();
            }
            Message::AddOnlineOpen => {
                self.adding_online = true;
                return Task::none();
            }
            Message::AddOnlineClose => {
                self.adding_online = false;
                return Task::none();
            }
            Message::AddCustomImapSmtp => {
                self.adding_online = false;
                self.custom_form = Some(MailForm::new());
                return Task::none();
            }
            Message::CustomFieldChanged(field, value) => {
                if let Some(form) = &mut self.custom_form {
                    match field {
                        FormField::Email => form.email = value,
                        FormField::Username => form.username = value,
                        FormField::ImapHost => form.imap_host = value,
                        FormField::ImapPort => form.imap_port = value,
                        FormField::SmtpHost => form.smtp_host = value,
                        FormField::SmtpPort => form.smtp_port = value,
                        FormField::Password => form.password = Password(value),
                        _ => {}
                    }
                }
                return Task::none();
            }
            Message::CustomStartTlsChanged(enabled) => {
                if let Some(form) = &mut self.custom_form {
                    let default = if form.smtp_starttls { "587" } else { "465" };
                    if form.smtp_port == default {
                        form.smtp_port = if enabled { "587" } else { "465" }.into();
                    }
                    form.smtp_starttls = enabled;
                }
                return Task::none();
            }
            Message::CancelCustomForm => {
                self.custom_form = None;
                return Task::none();
            }
            Message::SubmitCustomImapSmtp => {
                let Some(form) = self.custom_form.clone() else {
                    return Task::none();
                };
                let (settings, username, password) = match form.validate() {
                    Ok(validated) => validated,
                    Err(error) => return notify(error.text()),
                };
                self.busy = true;
                return Task::perform(
                    online::add_imap_smtp(settings, username, password),
                    |result| Message::CustomImapSmtpAdded(result.map_err(Error::from)),
                );
            }
            Message::CustomImapSmtpAdded(result) => {
                self.busy = false;
                let alert = match result {
                    Ok(_) => {
                        self.custom_form = None;
                        Some(fl!("account-save-success"))
                    }
                    Err(error) => Some(error.text()),
                };
                let mut tasks = vec![Task::perform(online::managed(), Message::Discovered)];
                if let Some(message) = alert {
                    tasks.push(notify(message));
                }
                return Task::batch(tasks);
            }
            Message::AuthStarted(result) => match result {
                Ok(request) => {
                    self.auth = Some(request.clone());
                    self.adding_online = false;
                    self.busy = true;
                    let mut tasks =
                        vec![Task::perform(online::complete(request.clone()), |result| {
                            Message::AuthCompleted(result)
                        })];
                    if let Err(error) = accounts::ui::open_sign_in_url(&request) {
                        tasks.push(notify(Error::from(error).text()));
                    }
                    return Task::batch(tasks);
                }
                Err(error) => {
                    self.auth = None;
                    // Keep the provider picker open: a failed sign-in attempt
                    // should let the user retry instead of dropping back to
                    // the empty account overview.
                    self.busy = false;
                    return notify(error.text());
                }
            },
            Message::OpenBrowser => {
                if let Some(request) = &self.auth
                    && let Err(error) = accounts::ui::open_sign_in_url(request)
                {
                    return notify(Error::from(error).text());
                }
                return Task::none();
            }
            Message::AuthCompleted(result) => {
                self.auth = None;
                self.adding_online = false;
                self.busy = false;
                let alert = match result {
                    Ok(_) => Some(fl!("account-save-success")),
                    Err(Error::OnlineCancelled) => None,
                    Err(error) => Some(error.text()),
                };
                let mut tasks = vec![Task::perform(online::managed(), Message::Discovered)];
                if let Some(message) = alert {
                    tasks.push(notify(message));
                }
                return Task::batch(tasks);
            }
            Message::CancelAuth(state) => {
                online::cancel(&state);
                self.auth = None;
                self.adding_online = false;
                self.busy = false;
                return Task::perform(online::managed(), Message::Discovered);
            }
            Message::RemovedOnline(result) => {
                self.busy = false;
                let message = match result {
                    Ok(()) => fl!("account-remove-success"),
                    Err(error) => error.text(),
                };
                return Task::batch([
                    Task::perform(online::managed(), Message::Discovered),
                    notify(message),
                ]);
            }
            Message::ToggleFinished(result) => {
                self.busy = false;
                let mut tasks = vec![Task::perform(online::managed(), Message::Discovered)];
                match result {
                    Ok((id, enabled)) => self.set_online_usable(id, enabled),
                    Err(error) => tasks.push(notify(error.text())),
                }
                return Task::batch(tasks);
            }
            Message::AddProvider(provider) if !self.busy => {
                self.busy = true;
                return Task::perform(online::begin(provider), Message::AuthStarted);
            }
            Message::Reconnect(id) if !self.busy => {
                self.busy = true;
                return Task::perform(online::reconnect(id), Message::AuthStarted);
            }
            Message::RemoveOnline(id) if !self.busy => {
                self.busy = true;
                return Task::perform(online::remove(id), Message::RemovedOnline);
            }
            Message::ToggleLink(id, enabled) if !self.busy => {
                self.set_online_usable(id, enabled);
                self.busy = true;
                return Task::perform(
                    async move {
                        online::set_enabled(id, enabled)
                            .await
                            .map(|()| (id, enabled))
                    },
                    Message::ToggleFinished,
                );
            }
            _ => {}
        }
        Task::none()
    }
}
