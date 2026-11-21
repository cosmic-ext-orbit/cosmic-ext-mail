// SPDX-License-Identifier: GPL-3.0-only

use super::{Folder, Mail, MailId};
use crate::features::accounts::Account;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, DirBuilder},
    io::{self, BufReader, BufWriter, Write},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub messages: Vec<Mail>,
}

pub(super) struct Cache(PathBuf);

impl Cache {
    pub fn for_account(account: &Account) -> Option<Self> {
        let root = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        // Account edits preserve the UUID, so scope cached data to the IMAP identity too.
        let identity =
            serde_json::to_vec(&(&account.imap_host, account.imap_port, &account.username)).ok()?;
        Some(Self(
            root.join("dev.edfloreshz.Mail")
                .join("mail-v1")
                .join(account.id.to_string())
                .join(format!("{:x}", Sha256::digest(identity))),
        ))
    }

    fn snapshot_path(&self, folder: Folder) -> PathBuf {
        self.0.join(match folder {
            Folder::Inbox => "inbox.json",
            Folder::Drafts => "drafts.json",
            Folder::Sent => "sent.json",
            Folder::Archive => "archive.json",
            Folder::Trash => "trash.json",
        })
    }

    pub fn snapshot(&self, folder: Folder) -> Option<Snapshot> {
        let file = fs::File::open(self.snapshot_path(folder)).ok()?;
        serde_json::from_reader(BufReader::new(file)).ok()
    }

    pub fn save_snapshot(&self, folder: Folder, snapshot: &Snapshot) -> io::Result<()> {
        atomic_write(&self.snapshot_path(folder), |writer| {
            serde_json::to_writer(writer, snapshot).map_err(io::Error::other)
        })
    }

    pub fn sync_lock(&self, folder: Folder) -> io::Result<fs::File> {
        private_dir(&self.0)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.snapshot_path(folder).with_extension("lock"))?;
        file.lock()?;
        Ok(file)
    }

    pub fn body_path(&self, id: &MailId) -> PathBuf {
        self.0
            .join("bodies")
            .join(format!("{:x}", Sha256::digest(id.folder.as_bytes())))
            .join(id.uid_validity.to_string())
            .join(id.uid.to_string())
    }
}

pub(super) fn save_body(path: &Path, body: &[u8]) -> io::Result<()> {
    atomic_write(path, |writer| writer.write_all(body))
}

fn private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path.parent().expect("cache file has a parent");
    private_dir(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = BufWriter::new(file.as_file_mut());
    write(&mut writer)?;
    writer.flush()?;
    drop(writer);
    file.persist(path)?;
    Ok(())
}
