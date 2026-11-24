// SPDX-License-Identifier: GPL-3.0-only

use lettre::Address;
use serde::{Deserialize, Serialize};
use std::fmt;
use url::Host;
use uuid::Uuid;

use super::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: Uuid,
    pub email: String,
    pub username: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_starttls: bool,
    #[serde(default)]
    pub online: bool,
}

#[derive(Clone, Default)]
pub struct Password(pub String);

impl fmt::Debug for Password {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Password([redacted])")
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Field {
    Email,
    Username,
    ImapHost,
    ImapPort,
    SmtpHost,
    SmtpPort,
}

pub(super) struct Form {
    pub(super) id: Option<Uuid>,
    pub(super) email: String,
    pub(super) username: String,
    pub(super) imap_host: String,
    pub(super) imap_port: String,
    pub(super) smtp_host: String,
    pub(super) smtp_port: String,
    pub(super) smtp_starttls: bool,
    pub(super) password: Password,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            id: None,
            email: String::new(),
            username: String::new(),
            imap_host: String::new(),
            imap_port: "993".into(),
            smtp_host: String::new(),
            smtp_port: "587".into(),
            smtp_starttls: true,
            password: Password::default(),
        }
    }
}

impl Form {
    pub(super) fn edit(account: &Account) -> Self {
        Self {
            id: Some(account.id),
            email: account.email.clone(),
            username: account.username.clone(),
            imap_host: account.imap_host.clone(),
            imap_port: account.imap_port.to_string(),
            smtp_host: account.smtp_host.clone(),
            smtp_port: account.smtp_port.to_string(),
            smtp_starttls: account.smtp_starttls,
            password: Password::default(),
        }
    }

    pub(super) fn change(&mut self, field: Field, value: String) {
        match field {
            Field::Email => self.email = value,
            Field::Username => self.username = value,
            Field::ImapHost => self.imap_host = value,
            Field::ImapPort => self.imap_port = value,
            Field::SmtpHost => self.smtp_host = value,
            Field::SmtpPort => self.smtp_port = value,
        }
    }

    pub(super) fn validate(&self) -> Result<Account, Error> {
        let email = self.email.trim();
        email.parse::<Address>().map_err(|_| Error::Email)?;
        let username = self.username.trim();
        if username.contains(['\r', '\n', '\0']) {
            return Err(Error::Username);
        }
        if self.id.is_none() && self.password.0.is_empty() {
            return Err(Error::Password);
        }
        if self.password.0.contains(['\r', '\n', '\0']) {
            return Err(Error::Password);
        }
        Ok(Account {
            id: self.id.unwrap_or_else(Uuid::new_v4),
            email: email.to_owned(),
            username: if username.is_empty() { email } else { username }.to_owned(),
            imap_host: host(&self.imap_host).ok_or(Error::ImapSettings)?,
            imap_port: port(&self.imap_port).ok_or(Error::ImapSettings)?,
            smtp_host: host(&self.smtp_host).ok_or(Error::SmtpSettings)?,
            smtp_port: port(&self.smtp_port).ok_or(Error::SmtpSettings)?,
            smtp_starttls: self.smtp_starttls,
            online: false,
        })
    }
}

fn host(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.contains(['/', '\\', '@', '?', '#', '%']) {
        return None;
    }
    match Host::parse(value).ok()? {
        Host::Domain(host) => Some(host),
        Host::Ipv4(address) => Some(address.to_string()),
        Host::Ipv6(address) => Some(address.to_string()),
    }
}

fn port(value: &str) -> Option<u16> {
    value.trim().parse().ok().filter(|&value| value != 0)
}
