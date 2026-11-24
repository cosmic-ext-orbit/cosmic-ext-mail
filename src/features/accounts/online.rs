// SPDX-License-Identifier: GPL-3.0-only

use accounts::{AccountsClient, MailCredentials};
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
