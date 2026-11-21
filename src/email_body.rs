// SPDX-License-Identifier: GPL-3.0-only

use base64::{Engine, engine::general_purpose::STANDARD};
use lol_html::{RewriteStrSettings, element, rewrite_str};
use mail_parser::{MessageParser, MessagePart};
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
        let mut images = HashMap::new();
        let html = match self {
            Self::Html(html) => html.to_owned(),
            Self::PlainText(text) => plain_text_html(text),
            Self::Mime(raw) => {
                let message = MessageParser::new()
                    .parse(raw)
                    .ok_or("Could not parse the MIME message")?;
                for part in message.root().parts() {
                    if let Some(id) = part.content_id()
                        && let Some(image) = inline_image(part)
                    {
                        images.entry(id.to_owned()).or_insert(image);
                    }
                }

                let html = message.html_bodies().collect::<Vec<_>>().join("\n");
                if html.is_empty() {
                    let text = message.text_bodies().collect::<Vec<_>>().join("\n");
                    plain_text_html(&text)
                } else {
                    html
                }
            }
        };

        let settings = RewriteStrSettings::new()
            .append_element_content_handler(element!(
                "script, iframe, frame, object, embed, link, base, meta[http-equiv]",
                |element| {
                    element.remove();
                    Ok(())
                }
            ))
            .append_element_content_handler(element!("img[src]", |element| {
                if let Some(src) = element.get_attribute("src")
                    && let Some((scheme, id)) = src.trim().split_once(':')
                    && scheme.eq_ignore_ascii_case("cid")
                {
                    element.remove_attribute("src");
                    if let Ok(id) = urlencoding::decode(id)
                        && let Some(image) = images.get(id.as_ref())
                    {
                        element.set_attribute("src", image)?;
                    }
                }
                Ok(())
            }));
        rewrite_str(&html, settings)
            .map_err(|error| format!("Could not prepare the email body: {error}"))
    }
}

fn plain_text_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><pre style="white-space:pre-wrap;overflow-wrap:anywhere">{escaped}</pre>"#
    )
}

fn inline_image(part: MessagePart<'_>) -> Option<String> {
    const MAX_INLINE_IMAGE_BYTES: usize = 5 * 1024 * 1024;
    let content_type = part.content_type()?;
    if !content_type.ctype().eq_ignore_ascii_case("image") {
        return None;
    }
    let subtype = content_type.subtype()?.to_ascii_lowercase();
    if !matches!(subtype.as_str(), "png" | "jpeg" | "gif" | "webp")
        || part.raw_body().len() > MAX_INLINE_IMAGE_BYTES * 2
    {
        return None;
    }
    let bytes = part.decoded();
    if bytes.len() > MAX_INLINE_IMAGE_BYTES {
        return None;
    }
    Some(format!(
        "data:image/{subtype};base64,{}",
        STANDARD.encode(bytes)
    ))
}

#[cfg(test)]
mod tests {
    use super::EmailBody;

    #[test]
    fn plain_text_is_rendered_literal() {
        let html = EmailBody::PlainText("a < b & c\nhello")
            .into_html()
            .unwrap();
        assert!(html.contains("a &lt; b &amp; c\nhello"));
    }

    #[test]
    fn mime_decodes_html_and_resolves_only_matching_inline_images() {
        let raw = concat!(
            "From: Sender <sender@example.invalid>\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/related; boundary=message\r\n\r\n",
            "--message\r\nContent-Type: text/html; charset=iso-8859-1\r\n",
            "Content-Transfer-Encoding: quoted-printable\r\n\r\n",
            "<p>caf=E9</p><img src=3D\"cid:logo%40example.invalid\">",
            "<img src=3D\"cid:missing\">\r\n",
            "--message\r\nContent-Type: image/png\r\n",
            "Content-ID: <logo@example.invalid>\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "aW1hZ2U=\r\n--message--\r\n",
        );
        let html = EmailBody::Mime(raw.as_bytes()).into_html().unwrap();
        assert!(html.contains("café"));
        assert!(html.contains("data:image/png;base64,aW1hZ2U="));
        assert!(!html.contains("cid:"));
    }

    #[test]
    fn html_keeps_formatting_and_removes_active_elements() {
        let html = EmailBody::Html(concat!(
            "<meta http-equiv=refresh content='0;url=https://example.invalid'>",
            "<link rel=preconnect href='https://example.invalid'>",
            "<script>window.close()</script><iframe></iframe>",
            "<table><tr><td style='color:red'>Receipt</td></tr></table>",
        ))
        .into_html()
        .unwrap();
        assert_eq!(
            html,
            "<table><tr><td style='color:red'>Receipt</td></tr></table>"
        );
    }
}
