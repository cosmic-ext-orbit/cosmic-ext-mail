// SPDX-License-Identifier: GPL-3.0-only

mod html;
mod mime;

use std::{collections::HashMap, sync::Arc};

/// The displayable content of a message, either decoded or as raw MIME bytes.
#[derive(Clone, Copy, Debug)]
pub enum EmailBody<'a> {
    Html(&'a str),
    PlainText(&'a str),
    Mime(&'a [u8]),
}

/// Sanitized message content ready to display in an email preview.
#[derive(Debug, Clone)]
pub struct PreparedEmail(pub(super) Arc<str>);

impl PreparedEmail {
    /// Prepares an already parsed message, including its inline images.
    pub fn from_message(message: &mail_parser::Message<'_>) -> Result<Self, String> {
        let (html, inline_images) = mime::decode(message);
        html::prepare(&html, &inline_images).map(|html| Self(html.into()))
    }
}

impl EmailBody<'_> {
    pub(super) fn prepare(self) -> Result<PreparedEmail, String> {
        match self {
            Self::Html(html) => {
                html::prepare(html, &HashMap::new()).map(|html| PreparedEmail(html.into()))
            }
            Self::PlainText(text) => html::prepare(&html::plain_text_html(text), &HashMap::new())
                .map(|html| PreparedEmail(html.into())),
            Self::Mime(raw) => {
                let message = mail_parser::MessageParser::new()
                    .parse(raw)
                    .ok_or("Could not parse the MIME message")?;
                PreparedEmail::from_message(&message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_and_raw_mime_use_the_same_sanitized_preview() {
        let raw = b"Content-Type: text/html\r\n\r\n<p>Message</p><script>example()</script>";
        let parsed = mail_parser::MessageParser::new().parse(raw).unwrap();
        let prepared = PreparedEmail::from_message(&parsed).unwrap();
        let direct = EmailBody::Mime(raw).prepare().unwrap();
        assert_eq!(prepared.0, direct.0);
        assert!(prepared.0.contains("<p>Message</p>"));
        assert!(!prepared.0.contains("example()"));
    }
}
