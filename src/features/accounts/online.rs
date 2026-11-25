// SPDX-License-Identifier: GPL-3.0-only

use accounts::{
    Account as ManagedAccount, Accounts, AuthRequest, Config, Endpoint, Event, ImapSmtpSettings,
    MailConnectionSettings, MailCredentials, Provider, Resource, SecretString, Service,
};
use cosmic::iced::{Subscription, futures::stream};
use lettre::Address;
use tokio::sync::{OnceCell, broadcast::error::RecvError};
use uuid::Uuid;

use super::{Account, Error, Password, connection::Authentication};

const APP_ID: &str = "dev.edfloreshz.Mail";
static MANAGER: OnceCell<Accounts> = OnceCell::const_new();

pub(super) async fn manager() -> Result<&'static Accounts, Error> {
    MANAGER
        .get_or_try_init(|| async {
            let config = Config::new(APP_ID)
                .with_client_from_env(Provider::Google, Service::Mail)
                .with_client_from_env(Provider::Microsoft, Service::Mail)
                .with_resource_client_from_env(
                    Provider::Microsoft,
                    Service::Mail,
                    Resource::Exchange,
                );
            Accounts::new(config).await
        })
        .await
        .map_err(Error::from)
}

fn provider_settings(provider: Provider) -> MailConnectionSettings {
    match provider {
        Provider::Google => MailConnectionSettings {
            transport: accounts::MailTransport::ImapSmtp,
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            imap_tls: true,
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 587,
            smtp_starttls: true,
        },
        _ => MailConnectionSettings {
            transport: accounts::MailTransport::ImapSmtp,
            imap_host: "outlook.office365.com".into(),
            imap_port: 993,
            imap_tls: true,
            smtp_host: "smtp.office365.com".into(),
            smtp_port: 587,
            smtp_starttls: true,
        },
    }
}

fn convert(account: &ManagedAccount) -> Option<Account> {
    let email = account.email_or_identity();
    if email.chars().any(char::is_control) || email.parse::<Address>().is_err() {
        return None;
    }
    if account.provider == Provider::Microsoft {
        return Some(Account {
            id: account.id,
            email: email.to_owned(),
            username: email.to_owned(),
            imap_host: String::new(),
            imap_port: 0,
            online: true,
            microsoft_graph: true,
        });
    }
    let settings = match &account.endpoint {
        Endpoint::ImapSmtp(settings) => MailConnectionSettings {
            transport: accounts::MailTransport::ImapSmtp,
            imap_host: settings.imap_host.clone(),
            imap_port: settings.imap_port,
            imap_tls: settings.imap_tls,
            smtp_host: settings.smtp_host.clone(),
            smtp_port: settings.smtp_port,
            smtp_starttls: settings.smtp_starttls,
        },
        _ => provider_settings(account.provider),
    };
    Some(Account {
        id: account.id,
        email: email.to_owned(),
        username: email.to_owned(),
        imap_host: settings.imap_host,
        imap_port: settings.imap_port,
        online: true,
        microsoft_graph: false,
    })
}

/// Returns all app-owned accounts exposing the Mail service.
pub(super) async fn managed() -> Result<Vec<ManagedAccount>, Error> {
    Ok(manager().await?.list(Service::Mail).await)
}

pub(super) fn enabled_from(managed: &[ManagedAccount]) -> Vec<Account> {
    let mut accounts: Vec<Account> = managed
        .iter()
        .filter(|account| account.enabled)
        .filter_map(convert)
        .collect();
    accounts.sort_by(|a, b| a.email.cmp(&b.email));
    accounts
}

/// Returns the Mail accounts that are currently enabled.
pub(super) async fn available() -> Result<Vec<Account>, Error> {
    Ok(enabled_from(&managed().await?))
}

/// IDs of embedded accounts whose Mail service is currently usable.
pub(crate) async fn enabled_ids() -> Result<Vec<Uuid>, Error> {
    Ok(available()
        .await?
        .into_iter()
        .map(|account| account.id)
        .collect())
}

pub(super) async fn set_enabled(id: Uuid, enabled: bool) -> Result<(), Error> {
    manager().await?.set_enabled(id, enabled).await?;
    Ok(())
}

pub(crate) async fn graph_token(id: Uuid, refresh: bool) -> Result<Password, Error> {
    let token = manager()
        .await?
        .access_token(id, Service::Mail, refresh)
        .await?;
    Ok(Password(token))
}

pub(super) async fn begin(provider: Provider) -> Result<AuthRequest, Error> {
    let manager = manager().await?;
    accounts::ui::begin_sign_in(manager, provider, Service::Mail)
        .await
        .map_err(Error::from)
}

pub(super) async fn complete(request: AuthRequest) -> Result<ManagedAccount, Error> {
    manager()
        .await?
        .complete_auth(request)
        .await
        .map_err(Error::from)
}

pub(super) async fn add_imap_smtp(
    settings: ImapSmtpSettings,
    username: String,
    password: SecretString,
) -> Result<ManagedAccount, Error> {
    manager()
        .await?
        .add_imap_smtp_account(settings, username, password)
        .await
        .map_err(Error::from)
}

pub(super) async fn reconnect(id: Uuid) -> Result<AuthRequest, Error> {
    manager().await?.reconnect(id).await.map_err(Error::from)
}

pub(super) fn cancel(state: &str) {
    if let Some(manager) = MANAGER.get() {
        let state = state.to_owned();
        tokio::spawn(async move {
            manager.cancel_auth(&state).await;
        });
    }
}

pub(super) async fn remove(id: Uuid) -> Result<(), Error> {
    manager().await?.remove(id).await.map_err(Error::from)
}

/// Watches the embedded manager for Mail account changes.
pub(crate) fn changes() -> Subscription<()> {
    Subscription::run_with((), |()| {
        stream::unfold((), |()| async move {
            match manager().await {
                Ok(accounts) => {
                    let mut events = accounts.subscribe();
                    loop {
                        match events.recv().await {
                            Ok(Event::AccountsChanged(Service::Mail))
                            | Err(RecvError::Lagged(_)) => break,
                            Ok(_) => continue,
                            Err(RecvError::Closed) => {
                                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                                break;
                            }
                        }
                    }
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_secs(30)).await,
            }
            Some(((), ()))
        })
    })
}

/// Returns the mail access credentials for the given account ID.
pub(super) async fn mail_access(id: Uuid, refresh: bool) -> Result<Authentication, Error> {
    let manager = manager().await?;
    if refresh {
        let account = manager.get(id).await.ok_or(Error::AccountMissing)?;
        let resource = match account.provider {
            Provider::Microsoft => Resource::Exchange,
            Provider::Google => Resource::Primary,
            _ => Resource::ImapSmtp,
        };
        manager.refresh(id, resource).await?;
    }
    let credentials = manager.mail_access(id).await?;
    Ok(match credentials {
        MailCredentials::OAuth {
            settings, token, ..
        } => Authentication::OAuth(settings, Password(token.expose().to_owned())),
        MailCredentials::Password {
            settings,
            username,
            password,
        } => Authentication::ManagedPassword(
            settings,
            username,
            Password(password.expose().to_owned()),
        ),
    })
}
