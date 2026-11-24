// SPDX-License-Identifier: GPL-3.0-only

use accounts::{AccountsClient, MailCredentials, models::Service};
use cosmic::iced::{
    Subscription,
    futures::{StreamExt, stream},
};
use lettre::Address;
use uuid::Uuid;

use super::{Account, Error, Password, connection::Authentication};

// Mail's transport layer consumes the broker's settings directly.
pub(super) use accounts::MailConnectionSettings;

/// Returns a list of all available mail accounts, filtering out any invalid email addresses.
pub(super) async fn available() -> Result<Vec<Account>, Error> {
    let client = AccountsClient::connect().await.map_err(Error::from)?;
    let mail = client.list_mail_accounts().await;
    let graph = client.list_enabled_accounts(Service::Email).await;
    let mut accounts = Vec::new();
    let graph_ids: std::collections::HashSet<_> = graph
        .as_ref()
        .ok()
        .into_iter()
        .flatten()
        .filter(|account| account.provider == "microsoft")
        .filter_map(|account| {
            let email = valid_email(account)?;
            accounts.push(Account {
                id: account.id,
                email: email.to_owned(),
                username: email.to_owned(),
                imap_host: String::new(),
                imap_port: 0,
                smtp_host: String::new(),
                smtp_port: 0,
                smtp_starttls: false,
                online: true,
                microsoft_graph: true,
            });
            Some(account.id)
        })
        .collect();

    match mail {
        Ok(mail) => {
            for (account, settings) in mail {
                if graph_ids.contains(&account.id) {
                    continue;
                }
                let Some(email) = valid_email(&account) else {
                    continue;
                };
                let smtp_starttls = settings.smtp_starttls();
                accounts.push(Account {
                    id: account.id,
                    email: email.to_owned(),
                    username: email.to_owned(),
                    imap_host: settings.imap_host,
                    imap_port: settings.imap_port,
                    smtp_host: settings.smtp_host,
                    smtp_port: settings.smtp_port,
                    smtp_starttls,
                    online: true,
                    microsoft_graph: false,
                });
            }
        }
        Err(error) if accounts.is_empty() => return Err(Error::from(error)),
        Err(_) => {}
    }

    accounts.sort_by(|a, b| a.email.cmp(&b.email));
    Ok(accounts)
}

fn valid_email(account: &accounts::models::Account) -> Option<&str> {
    let email = account.email_or_identity();
    (!email.chars().any(char::is_control) && email.parse::<Address>().is_ok()).then_some(email)
}

/// IDs of broker accounts whose Email service is currently usable.
pub(crate) async fn enabled_ids() -> Result<Vec<Uuid>, Error> {
    Ok(available()
        .await?
        .into_iter()
        .map(|account| account.id)
        .collect())
}

pub(crate) async fn graph_token(id: Uuid, refresh: bool) -> Result<Password, Error> {
    let client = AccountsClient::connect().await.map_err(Error::from)?;
    // This is the provider's primary OAuth token. The IMAP/SMTP credential
    // API returns an Exchange token for Microsoft and must not be used here.
    let (token, _) = client
        .access_token(&id, &Service::Email, refresh)
        .await
        .map_err(Error::from)?;
    Ok(Password(token))
}

/// Watches the broker's global account enable/disable signal.
pub(crate) fn account_toggles() -> Subscription<()> {
    Subscription::run_with((), |_| {
        stream::unfold(
            None,
            |stream: Option<accounts::proxy::AccountToggledStream>| async move {
                let mut stream = match stream {
                    Some(stream) => stream,
                    None => {
                        let client = AccountsClient::connect().await.ok()?;
                        client.receive_account_toggled().await.ok()?
                    }
                };
                stream.next().await?;
                Some(((), Some(stream)))
            },
        )
    })
}

/// Watches per-service enable/disable signals for the saved online accounts.
pub(crate) fn service_changes(ids: Vec<Uuid>) -> Subscription<()> {
    Subscription::run_with(ids, |ids| {
        stream::unfold(ids.clone(), |ids| async move {
            let streams = service_streams(&ids).await;
            stream::select_all(streams).next().await?;
            Some(((), ids))
        })
    })
}

async fn service_streams(ids: &[Uuid]) -> Vec<accounts::proxy::ServicesChangedStream> {
    let Ok(client) = AccountsClient::connect().await else {
        return Vec::new();
    };
    let mut streams = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(stream) = client.receive_services_changed(id).await {
            streams.push(stream);
        }
    }
    streams
}

/// Returns the mail access credentials for the given account ID, optionally refreshing the credentials.
pub(super) async fn mail_access(id: Uuid, refresh: bool) -> Result<Authentication, Error> {
    let client = AccountsClient::connect().await.map_err(Error::from)?;
    let credentials = client
        .mail_credentials(&id, refresh)
        .await
        .map_err(Error::from)?;

    Ok(match credentials {
        MailCredentials::OAuth {
            settings, token, ..
        } => Authentication::OAuth(settings, Password(token)),
        MailCredentials::Password {
            settings,
            username,
            password,
        } => Authentication::ManagedPassword(settings, username, Password(password)),
    })
}
