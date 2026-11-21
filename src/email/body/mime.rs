// SPDX-License-Identifier: GPL-3.0-only

use super::html::plain_text_html;
use base64::{Engine, engine::general_purpose::STANDARD};
use mail_parser::{MessageParser, MessagePart};
use std::collections::HashMap;

pub(super) fn decode(raw: &[u8]) -> Result<(String, HashMap<String, String>), String> {
    let message = MessageParser::new()
        .parse(raw)
        .ok_or("Could not parse the MIME message")?;
    let mut images = HashMap::new();
    for part in message.root().parts() {
        let id = part.content_id();
        let location = part.content_location();
        if (id.is_some() || location.is_some())
            && let Some(image) = inline_image(part)
        {
            if let Some(id) = id {
                images
                    .entry(format!("cid:{}", id.trim().trim_matches(['<', '>'])))
                    .or_insert_with(|| image.clone());
            }
            if let Some(location) = location {
                images.entry(location.trim().to_owned()).or_insert(image);
            }
        }
    }

    let mut html = message.html_bodies().collect::<Vec<_>>().join("\n");
    if html.is_empty() {
        let text = message.text_bodies().collect::<Vec<_>>().join("\n");
        html = plain_text_html(&text);
    }

    Ok((html, images))
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
