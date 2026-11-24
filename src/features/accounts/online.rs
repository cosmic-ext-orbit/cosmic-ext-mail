// SPDX-License-Identifier: GPL-3.0-only

use lettre::Address;
use uuid::Uuid;
use zbus::{Connection, proxy, zvariant::OwnedObjectPath};

use super::{Account, Error, Password};

pub(super) const IMAP: (&str, u16) = ("imap.gmail.com", 993);
pub(super) const SMTP: (&str, u16) = ("smtp.gmail.com", 587);

#[proxy(
    default_service = "dev.edfloreshz.Accounts",
    default_path = "/dev/edfloreshz/Accounts/Manager",
    interface = "dev.edfloreshz.Accounts.Manager"
)]
trait Manager {
    fn list_accounts(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[proxy(
    default_service = "dev.edfloreshz.Accounts",
    interface = "dev.edfloreshz.Accounts.Account"
)]
trait OnlineAccount {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn provider_id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn email(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn enabled(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn enabled_services(&self) -> zbus::Result<Vec<String>>;
}

#[proxy(
    default_service = "dev.edfloreshz.Accounts",
    interface = "dev.edfloreshz.Accounts.Credentials"
)]
trait Credentials {
    fn get_access_token(&self, service: &str) -> zbus::Result<(String, i64)>;
    fn invalidate_token(&self) -> zbus::Result<()>;
}

pub(super) async fn available() -> Result<Vec<Account>, Error> {
    let connection = Connection::session().await.map_err(broker_error)?;
    let manager = ManagerProxy::new(&connection).await.map_err(broker_error)?;
    let mut accounts = Vec::new();

    for path in manager.list_accounts().await.map_err(broker_error)? {
        let account = OnlineAccountProxy::builder(&connection)
            .path(path)
            .map_err(broker_error)?
            .build()
            .await
            .map_err(broker_error)?;
        let Ok(Some(account)) = google_account(account).await else {
            continue;
        };
        accounts.push(account);
    }
    accounts.sort_by(|a, b| a.email.cmp(&b.email));
    Ok(accounts)
}

async fn google_account(account: OnlineAccountProxy<'_>) -> zbus::Result<Option<Account>> {
    if account.provider_id().await? != "google"
        || !account.enabled().await?
        || !account
            .enabled_services()
            .await?
            .iter()
            .any(|service| service == "Email")
    {
        return Ok(None);
    }
    let email = account.email().await?;
    if email.chars().any(char::is_control) || email.parse::<Address>().is_err() {
        return Ok(None);
    }
    let Ok(id) = Uuid::parse_str(&account.id().await?) else {
        return Ok(None);
    };

    Ok(Some(Account {
        id,
        email: email.clone(),
        username: email,
        imap_host: IMAP.0.into(),
        imap_port: IMAP.1,
        smtp_host: SMTP.0.into(),
        smtp_port: SMTP.1,
        smtp_starttls: true,
        online: true,
    }))
}

pub(super) async fn token(id: Uuid, refresh: bool) -> Result<Password, Error> {
    let connection = Connection::session().await.map_err(broker_error)?;
    let path = format!(
        "/dev/edfloreshz/Accounts/Accounts/{}",
        id.to_string().replace('-', "_")
    );
    let account = OnlineAccountProxy::builder(&connection)
        .path(path.as_str())
        .map_err(broker_error)?
        .build()
        .await
        .map_err(broker_error)?;
    if account.provider_id().await.map_err(broker_error)? != "google" {
        return Err(Error::OnlineDisabled);
    }
    let credentials = CredentialsProxy::builder(&connection)
        .path(path)
        .map_err(broker_error)?
        .build()
        .await
        .map_err(broker_error)?;
    if refresh {
        credentials.invalidate_token().await.map_err(broker_error)?;
    }
    let (token, _) = credentials
        .get_access_token("Email")
        .await
        .map_err(broker_error)?;
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err(Error::OnlineReconnect);
    }
    Ok(Password(token))
}

fn broker_error(error: zbus::Error) -> Error {
    match error {
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "dev.edfloreshz.Accounts.Error.NeedsReauth" => Error::OnlineReconnect,
            "dev.edfloreshz.Accounts.Error.AccessDenied"
            | "org.freedesktop.DBus.Error.AccessDenied" => Error::OnlineDenied,
            "dev.edfloreshz.Accounts.Error.ConsentTimeout" => Error::OnlineConsentTimeout,
            "dev.edfloreshz.Accounts.Error.Disabled"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownInterface" => Error::OnlineDisabled,
            _ => Error::OnlineUnavailable,
        },
        _ => Error::OnlineUnavailable,
    }
}
