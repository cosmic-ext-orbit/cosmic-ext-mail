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
    #[serde(default)]
    pub uid_validity: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct InboxCursor {
    uid_validity: u32,
    highest_uid: u32,
}

pub(super) struct Cache(PathBuf);

impl Cache {
    pub fn for_account(account: &Account) -> Option<Self> {
        let root = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        // Account edits preserve the UUID, so scope cached data to the IMAP identity too.
        let identity = if account.microsoft_graph {
            serde_json::to_vec(&("microsoft-graph", &account.email)).ok()?
        } else {
            serde_json::to_vec(&(&account.imap_host, account.imap_port, &account.username)).ok()?
        };
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
            Folder::Junk => "junk.json",
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

    pub fn new_inbox_messages(&self, snapshot: &Snapshot) -> io::Result<usize> {
        let Some(uid_validity) = snapshot.uid_validity else {
            return Ok(0);
        };
        let path = self.0.join("inbox-notification-cursor.json");
        let previous = fs::File::open(&path)
            .ok()
            .and_then(|file| serde_json::from_reader::<_, InboxCursor>(BufReader::new(file)).ok())
            .filter(|cursor| cursor.uid_validity == uid_validity);
        let mut highest_uid = previous.as_ref().map_or(0, |cursor| cursor.highest_uid);
        let mut count = 0;
        for mail in &snapshot.messages {
            highest_uid = highest_uid.max(mail.id.uid);
            if mail.unread
                && previous
                    .as_ref()
                    .is_some_and(|cursor| mail.id.uid > cursor.highest_uid)
            {
                count += 1;
            }
        }
        if previous
            .as_ref()
            .is_none_or(|cursor| highest_uid != cursor.highest_uid)
        {
            atomic_write(&path, |writer| {
                serde_json::to_writer(
                    writer,
                    &InboxCursor {
                        uid_validity,
                        highest_uid,
                    },
                )
                .map_err(io::Error::other)
            })?;
        }
        Ok(count)
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
        if let Some(graph_id) = &id.graph_id {
            return self
                .0
                .join("bodies")
                .join(format!("{:x}", Sha256::digest(graph_id.as_bytes())));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(uid_validity: u32, messages: &[(u32, bool)]) -> Snapshot {
        Snapshot {
            uid_validity: Some(uid_validity),
            messages: messages
                .iter()
                .map(|&(uid, unread)| Mail {
                    id: MailId {
                        account: uuid::Uuid::nil(),
                        folder: "INBOX".into(),
                        uid_validity,
                        uid,
                        graph_id: None,
                    },
                    account_email: String::new(),
                    unread,
                    flagged: false,
                    sender: String::new(),
                    address: String::new(),
                    recipients: None,
                    subject: String::new(),
                    time: String::new(),
                    timestamp: 0,
                    thread: None,
                })
                .collect(),
        }
    }

    #[test]
    fn inbox_cursor_tracks_new_unread_mail_across_empty_folders_and_resets() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache(dir.path().to_owned());
        assert_eq!(
            cache
                .new_inbox_messages(&snapshot(1, &[(5, true)]))
                .unwrap(),
            0
        );
        assert_eq!(cache.new_inbox_messages(&snapshot(1, &[])).unwrap(), 0);
        let recent = snapshot(1, &[(5, true), (6, true), (7, false)]);
        assert_eq!(cache.new_inbox_messages(&recent).unwrap(), 1);
        assert_eq!(
            Cache(dir.path().to_owned())
                .new_inbox_messages(&recent)
                .unwrap(),
            0
        );
        assert_eq!(
            cache
                .new_inbox_messages(&snapshot(2, &[(3, true)]))
                .unwrap(),
            0
        );
        assert_eq!(
            cache
                .new_inbox_messages(&snapshot(2, &[(4, true)]))
                .unwrap(),
            1
        );
    }
}
