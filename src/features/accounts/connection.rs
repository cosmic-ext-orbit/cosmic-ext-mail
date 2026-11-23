// SPDX-License-Identifier: GPL-3.0-only

use lettre::{SmtpTransport, transport::smtp::authentication::Credentials};
use native_tls::TlsConnector;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::{Account, Error, Password, store};

const TIMEOUT: Duration = Duration::from_secs(10);

type Session = imap::Session<native_tls::TlsStream<TcpStream>>;

pub(super) async fn check(account: Account, password: Password) -> Result<(), Error> {
    let password = if password.0.is_empty() {
        store::stored_password(account.id).await?
    } else {
        password
    };
    let check = blocking_network(move || {
        check_imap(&account, &password)?;
        check_smtp(&account, password)
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
    let connect = async {
        let password = store::stored_password(account.id).await?;
        blocking_network(move || login_imap(&account, &password)).await
    };
    let mut session = tokio::time::timeout(Duration::from_secs(45), connect)
        .await
        .map_err(|_| Error::ConnectionTimeout)??;
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

pub(in crate::features) fn smtp_transport(
    account: &Account,
    password: Password,
) -> Result<SmtpTransport, Error> {
    let builder = if account.smtp_starttls {
        SmtpTransport::starttls_relay(&account.smtp_host)
    } else {
        SmtpTransport::relay(&account.smtp_host)
    }
    .map_err(|_| Error::SmtpConnect)?;
    Ok(builder
        .port(account.smtp_port)
        .timeout(Some(TIMEOUT))
        .credentials(Credentials::new(account.username.clone(), password.0))
        .build())
}

fn check_smtp(account: &Account, password: Password) -> Result<(), Error> {
    match smtp_transport(account, password)?.test_connection() {
        Ok(true) => Ok(()),
        _ => Err(Error::SmtpConnect),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
