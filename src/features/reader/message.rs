// SPDX-License-Identifier: GPL-3.0-only

use super::Attachment;
use crate::features::mailbox::Contact;
use cosmic_ext_mail::PreparedEmail;
use mail_parser::MessageParser;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct ComposeDetails {
    pub reply_address: Option<String>,
    pub message_id: Option<String>,
    pub text: String,
}

impl ComposeDetails {
    pub fn parse(bytes: &[u8]) -> Self {
        Self::from_message(MessageParser::new().parse(bytes).as_ref())
    }

    fn from_message(message: Option<&mail_parser::Message<'_>>) -> Self {
        Self {
            reply_address: message
                .and_then(|message| message.reply_to())
                .and_then(|addresses| addresses.first())
                .and_then(|mailbox| mailbox.address())
                .map(str::to_owned),
            message_id: message
                .and_then(|message| message.message_id())
                .map(str::to_owned),
            text: message
                .and_then(|message| message.body_text(0))
                .map_or_else(String::new, std::borrow::Cow::into_owned),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedMessage {
    pub(super) details: ComposeDetails,
    pub(super) recipients: Vec<Contact>,
    pub(super) attachments: Vec<Attachment>,
    pub(super) attachment_source: Option<Arc<[u8]>>,
    pub(super) body: Result<PreparedEmail, String>,
}

impl PreparedMessage {
    pub(super) fn parse(source: Arc<[u8]>) -> Self {
        let message = MessageParser::new().parse(&source);
        let details = ComposeDetails::from_message(message.as_ref());
        let recipients = message
            .as_ref()
            .and_then(|message| message.to())
            .map_or_else(Vec::new, Contact::from_list);
        let attachments = message.as_ref().map_or_else(Vec::new, Attachment::list);
        let body = message
            .as_ref()
            .ok_or_else(|| "Could not parse the MIME message".to_owned())
            .and_then(PreparedEmail::from_message);
        let attachment_source = (!attachments.is_empty()).then_some(source);
        Self {
            details,
            recipients,
            attachments,
            attachment_source,
            body,
        }
    }
}
