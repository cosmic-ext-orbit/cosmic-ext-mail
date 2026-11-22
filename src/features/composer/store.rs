// SPDX-License-Identifier: GPL-3.0-only

use super::{Attachment, Draft};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub(super) const ATTACHMENT_LIMIT: u64 = 18 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Store(PathBuf);

impl Store {
    #[cfg(test)]
    pub(super) fn for_test(root: &Path) -> Self {
        Self(root.to_path_buf())
    }

    pub fn open() -> io::Result<Self> {
        let root = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or(io::ErrorKind::NotFound)?
            .join("dev.edfloreshz.Mail/drafts");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
        Ok(Self(root))
    }

    fn directory(&self, id: Uuid) -> PathBuf {
        self.0.join(id.to_string())
    }

    pub fn load(&self) -> io::Result<(Vec<Draft>, bool)> {
        let mut drafts = Vec::new();
        let mut unreadable = false;
        for entry in fs::read_dir(&self.0)? {
            let entry = entry?;
            let Ok(id) = entry.file_name().to_string_lossy().parse::<Uuid>() else {
                continue;
            };
            let file = entry.path().join("draft.json");
            match File::open(file) {
                Ok(file) => match serde_json::from_reader::<_, Draft>(io::BufReader::new(file)) {
                    Ok(draft) if draft.id == id => drafts.push(draft),
                    _ => unreadable = true,
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(_) => unreadable = true,
            }
        }
        drafts.sort_by_key(|draft| (std::cmp::Reverse(draft.modified), draft.id));
        Ok((drafts, unreadable))
    }

    pub fn save(&self, draft: &Draft) -> io::Result<()> {
        let dir = self.directory(draft.id);
        self.prepare(&dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer(&mut file, draft)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(dir.join("draft.json"))?;
        File::open(dir)?.sync_all()?;
        File::open(&self.0)?.sync_all()
    }

    pub fn remove(&self, id: Uuid) -> io::Result<()> {
        match fs::remove_dir_all(self.directory(id)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    pub fn import(
        &self,
        id: Uuid,
        paths: Vec<PathBuf>,
        mut budget: u64,
    ) -> io::Result<Vec<Attachment>> {
        let dir = self.directory(id);
        self.prepare(&dir)?;
        let mut imported = Vec::new();
        let result = (|| {
            for path in paths {
                let name = path
                    .file_name()
                    .ok_or(io::ErrorKind::InvalidInput)?
                    .to_string_lossy()
                    .into_owned();
                if !fs::metadata(&path)?.is_file() {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let mut source = File::open(path)?.take(budget + 1);
                let mut file = tempfile::NamedTempFile::new_in(&dir)?;
                let size = io::copy(&mut source, &mut file)?;
                if size > budget {
                    return Err(io::ErrorKind::FileTooLarge.into());
                }
                budget -= size;
                let attachment = Attachment {
                    id: Uuid::new_v4(),
                    name,
                    size,
                };
                file.as_file().sync_all()?;
                file.persist(self.attachment_path(id, attachment.id))?;
                imported.push(attachment);
            }
            Ok(())
        })();
        if let Err(error) = result {
            for attachment in imported {
                let _ = self.remove_attachment(id, attachment.id);
            }
            return Err(error);
        }
        Ok(imported)
    }

    pub fn read_attachment(&self, draft: Uuid, attachment: &Attachment) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        File::open(self.attachment_path(draft, attachment.id))?
            .take(attachment.size.min(ATTACHMENT_LIMIT) + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != attachment.size || attachment.size > ATTACHMENT_LIMIT {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(bytes)
    }

    pub fn remove_attachment(&self, draft: Uuid, attachment: Uuid) -> io::Result<()> {
        fs::remove_file(self.attachment_path(draft, attachment))
    }

    fn attachment_path(&self, draft: Uuid, attachment: Uuid) -> PathBuf {
        self.directory(draft).join(attachment.to_string())
    }

    fn prepare(&self, dir: &Path) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
}
