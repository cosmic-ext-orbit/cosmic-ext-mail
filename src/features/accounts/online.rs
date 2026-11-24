// SPDX-License-Identifier: GPL-3.0-only

use lettre::Address;
use uuid::Uuid;
use zbus::{Connection, proxy, zvariant::OwnedObjectPath};

use super::{Account, Error, Password};

#[derive(serde::Deserialize, zbus::zvariant::Type)]
pub(in crate::features) struct MailConnectionSettings {
    pub(super) imap_host: String,
    pub(super) imap_port: u16,
    imap_tls: String,
    pub(super) smtp_host: String,
    pub(super) smtp_port: u16,
    smtp_tls: String,
    authentication: String,
}

impl MailConnectionSettings {
    fn supported(&self) -> bool {
        self.imap_tls == "tls"
            && matches!(self.smtp_tls.as_str(), "tls" | "starttls")
            && self.authentication == "xoauth2"
            && !self.imap_host.is_empty()
            && !self.smtp_host.is_empty()
            && self.imap_port != 0
            && self.smtp_port != 0
    }

    pub(super) fn smtp_starttls(&self) -> bool {
        self.smtp_tls == "starttls"
    }
}

#[proxy(
    default_service = "dev.edfloreshz.Accounts",
    default_path = "/dev/edfloreshz/Accounts/Manager",
    interface = "dev.edfloreshz.Accounts.Manager"
)]
trait Manager {
    fn list_accounts(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    #[zbus(property)]
    fn mail_endpoint_version(&self) -> zbus::Result<u32>;
}

#[proxy(
    default_service = "dev.edfloreshz.Accounts",
    interface = "dev.edfloreshz.Accounts.Account"
)]
trait OnlineAccount {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn email(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn enabled(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn enabled_services(&self) -> zbus::Result<Vec<String>>;
}

#[proxy(
    default_service = "dev.edfloreshz.Accounts",
    interface = "dev.edfloreshz.Accounts.Endpoint.Mail"
)]
trait MailEndpoint {
    fn get_connection_settings(&self) -> zbus::Result<MailConnectionSettings>;
}

#[proxy(
    default_service = "dev.edfloreshz.Accounts",
    interface = "dev.edfloreshz.Accounts.Credentials"
)]
trait Credentials {
    fn get_mail_access(&self) -> zbus::Result<(MailConnectionSettings, String, i64)>;
    fn invalidate_token(&self) -> zbus::Result<()>;
}

pub(super) async fn available() -> Result<Vec<Account>, Error> {
    let connection = Connection::session().await.map_err(broker_error)?;
    let manager = ManagerProxy::new(&connection).await.map_err(broker_error)?;
    let paths = manager.list_accounts().await.map_err(broker_error)?;
    let version = manager
        .mail_endpoint_version()
        .await
        .map_err(broker_error)?;
    if version == 0 {
        return Err(Error::OnlineUpgradeRequired);
    }

    let mut accounts = Vec::new();
    for path in paths {
        let Ok(Some(account)) = mail_account(&connection, path).await else {
            continue;
        };
        accounts.push(account);
    }
    accounts.sort_by(|a, b| a.email.cmp(&b.email));
    Ok(accounts)
}

async fn mail_account(
    connection: &Connection,
    path: OwnedObjectPath,
) -> zbus::Result<Option<Account>> {
    let account = OnlineAccountProxy::builder(connection)
        .path(path.clone())?
        .build()
        .await?;
    if !account.enabled().await?
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

    let settings = MailEndpointProxy::builder(connection)
        .path(path)?
        .build()
        .await?
        .get_connection_settings()
        .await?;
    if !settings.supported() {
        return Ok(None);
    }

    let smtp_starttls = settings.smtp_starttls();
    Ok(Some(Account {
        id,
        email: email.clone(),
        username: email,
        imap_host: settings.imap_host,
        imap_port: settings.imap_port,
        smtp_host: settings.smtp_host,
        smtp_port: settings.smtp_port,
        smtp_starttls,
        online: true,
    }))
}

pub(super) async fn mail_access(
    id: Uuid,
    refresh: bool,
) -> Result<(MailConnectionSettings, Password), Error> {
    let connection = Connection::session().await.map_err(broker_error)?;
    let path = format!(
        "/dev/edfloreshz/Accounts/Accounts/{}",
        id.to_string().replace('-', "_")
    );
    let credentials = CredentialsProxy::builder(&connection)
        .path(path)
        .map_err(broker_error)?
        .build()
        .await
        .map_err(broker_error)?;
    if refresh {
        credentials.invalidate_token().await.map_err(broker_error)?;
    }
    let (settings, token, _) = credentials.get_mail_access().await.map_err(broker_error)?;
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err(Error::OnlineReconnect);
    }
    if !settings.supported() {
        return Err(Error::OnlineDisabled);
    }
    Ok((settings, Password(token)))
}

fn broker_error(error: zbus::Error) -> Error {
    match error {
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "org.freedesktop.DBus.Error.UnknownProperty"
            | "org.freedesktop.DBus.Error.UnknownMethod" => Error::OnlineUpgradeRequired,
            "dev.edfloreshz.Accounts.Error.Failed" => Error::OnlineAccessFailed,
            "dev.edfloreshz.Accounts.Error.NeedsReauth" => Error::OnlineReconnect,
            "dev.edfloreshz.Accounts.Error.AccessDenied"
            | "org.freedesktop.DBus.Error.AccessDenied" => Error::OnlineDenied,
            "dev.edfloreshz.Accounts.Error.ConsentTimeout" => Error::OnlineConsentTimeout,
            "dev.edfloreshz.Accounts.Error.Disabled"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownInterface" => Error::OnlineDisabled,
            _ => Error::OnlineUnavailable,
        },
        zbus::Error::FDO(error)
            if matches!(
                *error,
                zbus::fdo::Error::UnknownProperty(_) | zbus::fdo::Error::UnknownMethod(_)
            ) =>
        {
            Error::OnlineUpgradeRequired
        }
        _ => Error::OnlineUnavailable,
    }
}
