// SPDX-License-Identifier: GPL-3.0-only

mod html;
mod mime;

use std::collections::HashMap;

/// The displayable content of a message, either decoded or as raw MIME bytes.
#[derive(Clone, Copy, Debug)]
pub enum EmailBody<'a> {
    Html(&'a str),
    PlainText(&'a str),
    Mime(&'a [u8]),
}

impl EmailBody<'_> {
    pub(crate) fn into_html(self) -> Result<String, String> {
        let (html, inline_images) = match self {
            Self::Html(html) => (html.to_owned(), HashMap::new()),
            Self::PlainText(text) => (html::plain_text_html(text), HashMap::new()),
            Self::Mime(raw) => mime::decode(raw)?,
        };
        html::prepare(&html, &inline_images)
    }
}
