// SPDX-License-Identifier: GPL-3.0-only

use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};
use secret_service::{EncryptionType, Item, SecretService};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use uuid::Uuid;

use super::{Account, Error, Password};

const APP_ID: &str = "dev.edfloreshz.Mail";
const CONFIG_KEY: &str = "accounts";

#[derive(Clone)]
pub(super) struct Store(Config);

impl Store {
    pub(super) fn open() -> Result<(Self, Vec<Account>), Error> {
        let store = Self(Config::new(APP_ID, 1).map_err(|_| Error::Settings)?);
        let accounts = store.load()?;
        Ok((store, accounts))
    }

    pub(super) fn load(&self) -> Result<Vec<Account>, Error> {
        match self.0.get_local(CONFIG_KEY) {
            Ok(accounts) => Ok(accounts),
            Err(cosmic::cosmic_config::Error::NotFound) => Ok(Vec::new()),
            Err(_) => Err(Error::Settings),
        }
    }

    fn lock(&self) -> Result<File, Error> {
        let directory = self.0.user_path().ok_or(Error::Settings)?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(".accounts.lock"))
            .map_err(|_| Error::Settings)?;
        match lock.try_lock() {
            Ok(()) => Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::StorageBusy),
            Err(std::fs::TryLockError::Error(_)) => Err(Error::Settings),
        }
    }

    pub(super) async fn save(
        &self,
        account: Account,
        editing: bool,
        password: Password,
    ) -> Result<Vec<Account>, Error> {
        let _lock = self.lock()?;
        let mut accounts = self.load()?;
        let existing = accounts.iter_mut().find(|saved| saved.id == account.id);
        if editing && existing.is_none() {
            return Err(Error::AccountMissing);
        }
        match existing {
            Some(saved) => *saved = account.clone(),
            None => accounts.push(account.clone()),
        }
        if password.0.is_empty() {
            self.write(&accounts)?;
            return Ok(accounts);
        }

        let service = connect().await?;
        let previous = if editing {
            read(&service, account.id).await?
        } else {
            None
        };
        put(&service, account.id, password.0.as_bytes()).await?;
        if let Err(error) = self.write(&accounts) {
            restore(&service, account.id, previous.as_deref()).await?;
            return Err(error);
        }
        Ok(accounts)
    }

    pub(super) async fn remove(&self, id: Uuid) -> Result<Vec<Account>, Error> {
        let _lock = self.lock()?;
        let mut accounts = self.load()?;
        if !accounts.iter().any(|account| account.id == id) {
            return Ok(accounts);
        }
        let service = connect().await?;
        let previous = read(&service, id).await?;
        delete(&service, id).await?;
        accounts.retain(|account| account.id != id);
        if let Err(error) = self.write(&accounts) {
            restore(&service, id, previous.as_deref()).await?;
            return Err(error);
        }
        Ok(accounts)
    }

    fn write(&self, accounts: &[Account]) -> Result<(), Error> {
        self.0
            .set(CONFIG_KEY, accounts)
            .map_err(|_| Error::Settings)
    }
}

pub(super) async fn stored_password(id: Uuid) -> Result<Password, Error> {
    let service = connect().await?;
    let bytes = read(&service, id).await?.ok_or(Error::PasswordMissing)?;
    String::from_utf8(bytes)
        .map(Password)
        .map_err(|_| Error::PasswordMissing)
}

fn attributes(id: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", APP_ID), ("account-id", id)])
}

async fn connect() -> Result<SecretService<'static>, Error> {
    SecretService::connect(EncryptionType::Dh)
        .await
        .map_err(keyring_error)
}

async fn item<'a>(service: &'a SecretService<'_>, id: Uuid) -> Result<Option<Item<'a>>, Error> {
    let found = service
        .search_items(attributes(&id.to_string()))
        .await
        .map_err(keyring_error)?;
    if let Some(item) = found.unlocked.into_iter().next() {
        return Ok(Some(item));
    }
    if let Some(item) = found.locked.into_iter().next() {
        item.unlock().await.map_err(keyring_error)?;
        return Ok(Some(item));
    }
    Ok(None)
}

async fn read(service: &SecretService<'_>, id: Uuid) -> Result<Option<Vec<u8>>, Error> {
    match item(service, id).await? {
        Some(item) => item.get_secret().await.map(Some).map_err(keyring_error),
        None => Ok(None),
    }
}

async fn put(service: &SecretService<'_>, id: Uuid, secret: &[u8]) -> Result<(), Error> {
    if let Some(item) = item(service, id).await? {
        return item
            .set_secret(secret, "text/plain")
            .await
            .map_err(keyring_error);
    }
    let collection = service
        .get_default_collection()
        .await
        .map_err(keyring_error)?;
    if collection.is_locked().await.map_err(keyring_error)? {
        collection.unlock().await.map_err(keyring_error)?;
    }
    collection
        .create_item(
            "COSMIC Mail account password",
            attributes(&id.to_string()),
            secret,
            true,
            "text/plain",
        )
        .await
        .map_err(keyring_error)?;
    Ok(())
}

async fn delete(service: &SecretService<'_>, id: Uuid) -> Result<(), Error> {
    if let Some(item) = item(service, id).await? {
        item.delete().await.map_err(keyring_error)?;
    }
    Ok(())
}

async fn restore(
    service: &SecretService<'_>,
    id: Uuid,
    previous: Option<&[u8]>,
) -> Result<(), Error> {
    match previous {
        Some(secret) => put(service, id, secret).await,
        None => delete(service, id).await,
    }
    .map_err(|_| Error::StorageRollback)
}

fn keyring_error(error: secret_service::Error) -> Error {
    match error {
        secret_service::Error::Locked | secret_service::Error::Prompt => Error::KeyringLocked,
        _ => Error::Keyring,
    }
}
