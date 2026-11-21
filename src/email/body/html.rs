// SPDX-License-Identifier: GPL-3.0-only

use lol_html::{RewriteStrSettings, element, rewrite_str};
use std::collections::HashMap;

pub(super) fn prepare(html: &str, images: &HashMap<String, String>) -> Result<String, String> {
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
    rewrite_str(html, settings)
        .map_err(|error| format!("Could not prepare the email body: {error}"))
}

pub(super) fn plain_text_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><pre style="white-space:pre-wrap;overflow-wrap:anywhere">{escaped}</pre>"#
    )
}
