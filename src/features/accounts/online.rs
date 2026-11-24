// SPDX-License-Identifier: GPL-3.0-only

use accounts::{AccountsClient, MailCredentials};
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
    let mut accounts = Vec::new();

    for (account, settings) in client.list_mail_accounts().await.map_err(Error::from)? {
        let email = account.email_or_identity();
        if email.chars().any(char::is_control) || email.parse::<Address>().is_err() {
            continue;
        }
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
        });
    }

    accounts.sort_by(|a, b| a.email.cmp(&b.email));
    Ok(accounts)
}

/// IDs of broker accounts whose Email service is currently usable.
pub(crate) async fn enabled_ids() -> Result<Vec<Uuid>, Error> {
    let client = AccountsClient::connect().await.map_err(Error::from)?;
    Ok(client
        .list_mail_accounts()
        .await
        .map_err(Error::from)?
        .into_iter()
        .map(|(account, _)| account.id)
        .collect())
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
