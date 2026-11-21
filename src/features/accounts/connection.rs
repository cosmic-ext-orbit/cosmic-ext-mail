// SPDX-License-Identifier: GPL-3.0-only

use lettre::{SmtpTransport, transport::smtp::authentication::Credentials};
use native_tls::TlsConnector;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::{Account, Error, Password, store};

const TIMEOUT: Duration = Duration::from_secs(10);

pub(super) async fn check(account: Account, password: Password) -> Result<(), Error> {
    let password = if password.0.is_empty() {
        store::stored_password(account.id).await?
    } else {
        password
    };
    let check = tokio::task::spawn_blocking(move || {
        check_imap(&account, &password)?;
        check_smtp(&account, password)
    });
    tokio::time::timeout(Duration::from_secs(45), check)
        .await
        .map_err(|_| Error::ConnectionTimeout)?
        .map_err(|_| Error::ConnectionTask)?
}

type Session = imap::Session<native_tls::TlsStream<TcpStream>>;

pub(in crate::features) async fn with_imap<T, E>(
    account: Account,
    operation: impl FnOnce(&mut Session) -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<Error> + Send + 'static,
{
    let work = async {
        let password = store::stored_password(account.id).await?;
        tokio::task::spawn_blocking(move || {
            let mut session = login_imap(&account, &password)?;
            operation(&mut session)
        })
        .await
        .map_err(|_| Error::ConnectionTask)?
    };
    tokio::time::timeout(Duration::from_secs(45), work)
        .await
        .map_err(|_| Error::ConnectionTimeout)?
}

fn check_imap(account: &Account, password: &Password) -> Result<(), Error> {
    let mut session = login_imap(account, password)?;
    let _ = session.logout();
    Ok(())
}

fn login_imap(account: &Account, password: &Password) -> Result<Session, Error> {
    let mut addresses = (account.imap_host.as_str(), account.imap_port)
        .to_socket_addrs()
        .map_err(|_| Error::ImapConnect)?;
    let stream = addresses
        .find_map(|addr| TcpStream::connect_timeout(&addr, TIMEOUT).ok())
        .ok_or(Error::ImapConnect)?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| Error::ImapConnect)?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| Error::ImapConnect)?;

    let tls = TlsConnector::builder()
        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
        .build()
        .map_err(|_| Error::ImapConnect)?;
    let stream = tls
        .connect(&account.imap_host, stream)
        .map_err(|_| Error::ImapConnect)?;
    let mut client = imap::Client::new(stream);
    client.read_greeting().map_err(|_| Error::ImapConnect)?;
    client
        .login(&account.username, &password.0)
        .map_err(|_| Error::ImapLogin)
}

fn check_smtp(account: &Account, password: Password) -> Result<(), Error> {
    let builder = if account.smtp_starttls {
        SmtpTransport::starttls_relay(&account.smtp_host)
    } else {
        SmtpTransport::relay(&account.smtp_host)
    }
    .map_err(|_| Error::SmtpConnect)?;
    let transport = builder
        .port(account.smtp_port)
        .timeout(Some(TIMEOUT))
        .credentials(Credentials::new(account.username.clone(), password.0))
        .build();
    match transport.test_connection() {
        Ok(true) => Ok(()),
        _ => Err(Error::SmtpConnect),
    }
}
