// SPDX-License-Identifier: GPL-3.0-only

use cosmic::iced::{Subscription, futures::stream, widget::image::Handle};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Cursor, sync::LazyLock, time::Duration};
use tokio::sync::Semaphore;

const MAX_RESPONSE_BYTES: usize = 128 * 1024;
static REQUESTS: Semaphore = Semaphore::const_new(4);
static CLIENT: LazyLock<reqwest::Result<reqwest::Client>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .build()
});

#[derive(Default)]
pub struct Avatars {
    images: HashMap<String, Option<Handle>>,
}

impl Avatars {
    pub fn get(&self, address: &str) -> Option<&Handle> {
        self.images.get(&normalize(address))?.as_ref()
    }

    pub fn save(&mut self, address: String, image: Option<Handle>) {
        self.images.insert(address, image);
    }

    pub fn subscription<'a>(
        &self,
        addresses: impl Iterator<Item = &'a str>,
    ) -> Subscription<(String, Option<Handle>)> {
        let mut seen = std::collections::HashSet::new();
        Subscription::batch(addresses.filter_map(|address| {
            let address = normalize(address);
            if address.is_empty()
                || self.images.contains_key(&address)
                || !seen.insert(address.clone())
            {
                return None;
            }
            Some(Subscription::run_with(address, |address| {
                let address = address.clone();
                stream::once(async move {
                    let photo = fetch(&address).await;
                    (address, photo)
                })
            }))
        }))
    }
}

fn normalize(address: &str) -> String {
    address.trim().to_lowercase()
}

async fn fetch(address: &str) -> Option<Handle> {
    let client = CLIENT.as_ref().ok()?;
    let _slot = REQUESTS.acquire().await.ok()?;
    let hash = Sha256::digest(address.as_bytes());
    let mut response = client
        .get(format!(
            "https://gravatar.com/avatar/{hash:x}?s=80&d=404&r=g"
        ))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }

    tokio::task::spawn_blocking(move || {
        let mut reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(128);
        limits.max_image_height = Some(128);
        limits.max_alloc = Some(2 * 1024 * 1024);
        reader.limits(limits);
        let image = reader.decode().ok()?.into_rgba8();
        Some(Handle::from_rgba(
            image.width(),
            image.height(),
            image.into_raw(),
        ))
    })
    .await
    .ok()
    .flatten()
}
