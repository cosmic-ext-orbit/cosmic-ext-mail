// SPDX-License-Identifier: GPL-3.0-only

use lettre::{
    SmtpTransport,
    transport::smtp::authentication::{Credentials, Mechanism},
};
use native_tls::TlsConnector;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::{Account, Error, Password, online, store};

const TIMEOUT: Duration = Duration::from_secs(10);

type Session = imap::Session<native_tls::TlsStream<TcpStream>>;

pub(in crate::features) enum Authentication {
    Password(Password),
    OAuth(Password),
}

pub(in crate::features) async fn authentication(
    account: &Account,
) -> Result<Authentication, Error> {
    if account.online {
        online::token(account.id, false)
            .await
            .map(Authentication::OAuth)
    } else {
        store::stored_password(account.id)
            .await
            .map(Authentication::Password)
    }
}

pub(super) async fn check(account: Account, password: Password) -> Result<(), Error> {
    let auth = if password.0.is_empty() {
        authentication(&account).await?
    } else {
        Authentication::Password(password)
    };
    let check = blocking_network(move || {
        let mut session = login_imap(&account, &auth)?;
        let _ = session.logout();
        match smtp_transport(&account, auth)?.test_connection() {
            Ok(true) => Ok(()),
            _ => Err(Error::SmtpConnect),
        }
    });
    tokio::time::timeout(Duration::from_secs(45), check)
        .await
        .map_err(|_| Error::ConnectionTimeout)?
}

pub(in crate::features) async fn with_imap<T, E>(
    account: Account,
    operation: impl FnOnce(&mut Session) -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<Error> + Send + 'static,
{
    tokio::time::timeout(
        Duration::from_secs(45),
        with_imap_session(account, operation),
    )
    .await
    .map_err(|_| Error::ConnectionTimeout)?
}

// Allow a folder sync to run many commands after connecting. The socket still
// enforces read and write timeouts for an unresponsive server.
pub(in crate::features) async fn with_imap_session<T, E>(
    account: Account,
    operation: impl FnOnce(&mut Session) -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<Error> + Send + 'static,
{
    let auth = authentication(&account).await?;
    let mut session = match connect_imap(account.clone(), auth).await {
        Err(Error::ImapLogin) if account.online => {
            let auth = Authentication::OAuth(online::token(account.id, true).await?);
            connect_imap(account, auth).await.map_err(|error| {
                if matches!(error, Error::ImapLogin) {
                    Error::OnlineReconnect
                } else {
                    error
                }
            })?
        }
        result => result?,
    };
    blocking_network(move || operation(&mut session)).await
}

// Tokio waits for its blocking pool on shutdown. A stuck mail server should
// not hold the GUI open after Quit; these bounded workers own their sockets.
async fn blocking_network<T, E>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<Error> + Send + 'static,
{
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);
    let permit = SLOTS.acquire().await.map_err(|_| Error::ConnectionTask)?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    std::thread::Builder::new()
        .name("mail-network".into())
        .spawn(move || {
            let _permit = permit;
            let _runtime = runtime.enter();
            if !send.is_closed() {
                let _ = send.send(work());
            }
        })
        .map_err(|_| Error::ConnectionTask)?;
    receive.await.map_err(|_| Error::ConnectionTask)?
}

async fn connect_imap(account: Account, auth: Authentication) -> Result<Session, Error> {
    tokio::time::timeout(
        Duration::from_secs(45),
        blocking_network(move || login_imap(&account, &auth)),
    )
    .await
    .map_err(|_| Error::ConnectionTimeout)?
}

struct Xoauth2(std::cell::Cell<Option<String>>);

impl imap::Authenticator for Xoauth2 {
    type Response = String;

    fn process(&self, _: &[u8]) -> Self::Response {
        self.0.take().unwrap_or_default()
    }
}

fn login_imap(account: &Account, auth: &Authentication) -> Result<Session, Error> {
    let (host, port) = if matches!(auth, Authentication::OAuth(_)) {
        online::IMAP
    } else {
        (account.imap_host.as_str(), account.imap_port)
    };
    let mut addresses = (host, port)
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
    let stream = tls.connect(host, stream).map_err(|_| Error::ImapConnect)?;
    let mut client = imap::Client::new(stream);
    client.read_greeting().map_err(|_| Error::ImapConnect)?;
    match auth {
        Authentication::Password(password) => client.login(&account.username, &password.0),
        Authentication::OAuth(token) => client.authenticate(
            "XOAUTH2",
            &Xoauth2(std::cell::Cell::new(Some(format!(
                "user={}\x01auth=Bearer {}\x01\x01",
                account.email, token.0
            )))),
        ),
    }
    .map_err(|_| Error::ImapLogin)
}

pub(in crate::features) fn smtp_transport(
    account: &Account,
    auth: Authentication,
) -> Result<SmtpTransport, Error> {
    let (builder, port) = if matches!(&auth, Authentication::OAuth(_)) {
        (
            SmtpTransport::starttls_relay(online::SMTP.0),
            online::SMTP.1,
        )
    } else if account.smtp_starttls {
        (
            SmtpTransport::starttls_relay(&account.smtp_host),
            account.smtp_port,
        )
    } else {
        (SmtpTransport::relay(&account.smtp_host), account.smtp_port)
    };
    let builder = builder
        .map_err(|_| Error::SmtpConnect)?
        .port(port)
        .timeout(Some(TIMEOUT));
    Ok(match auth {
        Authentication::Password(password) => {
            builder.credentials(Credentials::new(account.username.clone(), password.0))
        }
        Authentication::OAuth(token) => builder
            .authentication(vec![Mechanism::Xoauth2])
            .credentials(Credentials::new(account.email.clone(), token.0)),
    }
    .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xoauth2_sends_the_token_only_once() {
        use imap::Authenticator;

        let auth = Xoauth2(std::cell::Cell::new(Some("fixture-credentials".into())));
        assert_eq!(auth.process(b""), "fixture-credentials");
        assert!(auth.process(b"invalid token").is_empty());
    }

    #[test]
    fn shutting_down_tokio_does_not_wait_for_network_io() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (started, running) = std::sync::mpsc::channel();
        let (unblock, blocked) = std::sync::mpsc::channel();
        let (finished, stopped) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            let _ = blocking_network(move || -> Result<(), Error> {
                started.send(()).unwrap();
                let _ = blocked.recv_timeout(Duration::from_secs(5));
                finished.send(()).unwrap();
                Ok(())
            })
            .await;
        });
        running.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(runtime);
        assert!(matches!(
            stopped.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        unblock.send(()).unwrap();
        stopped.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
