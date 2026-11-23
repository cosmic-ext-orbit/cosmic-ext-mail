// SPDX-License-Identifier: GPL-3.0-only

use super::{Message, Reader};
use crate::fl;
use cosmic::{Task, dialog::file_chooser};
use mail_parser::MessageParser;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct Attachment {
    pub(super) name: String,
    pub(super) size: usize,
    part: usize,
}

impl Attachment {
    pub(super) fn list(message: &mail_parser::Message<'_>) -> Vec<Self> {
        message
            .attachments()
            .enumerate()
            .filter(|(_, part)| !part.is_inline())
            .map(|(index, part)| {
                let name = part
                    .attachment_name()
                    .and_then(|name| name.rsplit(['/', '\\']).next())
                    .map(|name| {
                        name.chars()
                            .filter(|ch| !ch.is_control())
                            .collect::<String>()
                    })
                    .and_then(|name| {
                        let name = name.trim();
                        (!matches!(name, "" | "." | "..")).then(|| name.to_owned())
                    })
                    .unwrap_or_else(|| fl!("mail-attachment-name", number = (index + 1)));
                Self {
                    name,
                    size: part.decoded_len(),
                    part: index,
                }
            })
            .collect()
    }
}

impl Reader {
    pub fn save_attachment(&self, request: u64, index: usize) -> Task<Message> {
        if request != self.request {
            return Task::none();
        }
        let (Some(file), Some(source)) =
            (self.attachments.get(index), self.attachment_source.as_ref())
        else {
            return Task::none();
        };
        let name = file.name.clone();
        let part = file.part;
        let source = Arc::clone(source);
        Task::perform(
            async move {
                let destination = match file_chooser::save::Dialog::new()
                    .title(fl!("mail-save-attachment"))
                    .file_name(name)
                    .save_file()
                    .await
                {
                    Ok(response) => {
                        let Some(url) = response.url() else {
                            return Ok(());
                        };
                        url.to_file_path().map_err(|_| ())?
                    }
                    Err(file_chooser::Error::Cancelled) => return Ok(()),
                    Err(_) => return Err(()),
                };
                tokio::task::spawn_blocking(move || {
                    let message = MessageParser::new().parse(&source).ok_or(())?;
                    let attachment = message.attachments().nth(part).ok_or(())?;
                    std::fs::write(destination, attachment.decoded()).map_err(|_| ())
                })
                .await
                .map_err(|_| ())?
            },
            Message::AttachmentSaved,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_list_excludes_inline_parts_and_preserves_the_part_index() {
        let raw = concat!(
            "From: sender@example.org\r\nMIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=mail\r\n\r\n",
            "--mail\r\nContent-Type: text/plain\r\n\r\nHello\r\n",
            "--mail\r\nContent-Type: image/png\r\n",
            "Content-Disposition: inline; filename=logo.png\r\n\r\ninline\r\n",
            "--mail\r\nContent-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=\"../report.bin\"\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--mail--\r\n",
        )
        .as_bytes();

        let message = MessageParser::new().parse(raw).expect("MIME message");
        let prepared = crate::features::reader::message::PreparedMessage::parse(Arc::from(raw));
        assert!(prepared.body.is_ok());
        assert_eq!(prepared.details.text.trim(), "Hello");
        assert_eq!(prepared.attachment_source.as_deref(), Some(raw));
        let attachments = prepared.attachments;
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].name, "report.bin");
        assert_eq!(attachments[0].size, 3);
        assert_eq!(
            message
                .attachments()
                .nth(attachments[0].part)
                .unwrap()
                .decoded()
                .as_ref(),
            &[0, 1, 2]
        );
    }
}
