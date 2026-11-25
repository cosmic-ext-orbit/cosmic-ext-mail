// SPDX-License-Identifier: GPL-3.0-only

use accounts::{ImapSmtpSettings, SecretString};
use lettre::Address;
use std::fmt;
use url::Host;
use uuid::Uuid;

use super::Error;

#[derive(Clone, Debug)]
pub struct Account {
    pub id: Uuid,
    pub email: String,
    pub username: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub online: bool,
    pub microsoft_graph: bool,
}

#[derive(Clone, Default)]
pub struct Password(pub String);

impl fmt::Debug for Password {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Password([redacted])")
    }
}

/// Host-side state for the shared Accounts custom mail form.
#[derive(Clone, Debug, Default)]
pub struct MailForm {
    pub email: String,
    pub username: String,
    pub imap_host: String,
    pub imap_port: String,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_starttls: bool,
    pub password: Password,
}

impl MailForm {
    pub fn new() -> Self {
        Self {
            imap_port: "993".into(),
            smtp_port: "587".into(),
            smtp_starttls: true,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(ImapSmtpSettings, String, SecretString), Error> {
        let email = self.email.trim();
        email.parse::<Address>().map_err(|_| Error::Email)?;

        let username = self.username.trim();
        if username.contains(['\r', '\n', '\0']) {
            return Err(Error::Username);
        }
        if self.password.0.contains(['\r', '\n', '\0']) {
            return Err(Error::Password);
        }

        Ok((
            ImapSmtpSettings {
                imap_host: host(&self.imap_host).ok_or(Error::ImapSettings)?,
                imap_port: port(&self.imap_port).ok_or(Error::ImapSettings)?,
                imap_tls: true,
                smtp_host: host(&self.smtp_host).ok_or(Error::SmtpSettings)?,
                smtp_port: port(&self.smtp_port).ok_or(Error::SmtpSettings)?,
                smtp_starttls: self.smtp_starttls,
            },
            if username.is_empty() {
                email.to_owned()
            } else {
                username.to_owned()
            },
            SecretString::new(self.password.0.clone()),
        ))
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
