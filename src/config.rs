// SPDX-License-Identifier: GPL-3.0-only

use crate::fl;
use serde::{Deserialize, Serialize};

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};

#[derive(Debug, Default, Clone, CosmicConfigEntry, Eq, PartialEq)]
#[version = 1]
pub struct Config {
    pub load_remote_images: bool,
    pub hide_footer: bool,
    pub show_recipient_pictures: bool,
    pub fetch_sender_photos: bool,
    pub message_actions: MessageActions,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum MessageActions {
    IconOnly,
    TextOnly,
    #[default]
    IconAndText,
}

impl MessageActions {
    pub const ALL: [Self; 3] = [Self::IconOnly, Self::TextOnly, Self::IconAndText];

    pub fn label(self) -> String {
        match self {
            Self::IconOnly => fl!("message-actions-icon-only"),
            Self::TextOnly => fl!("message-actions-text-only"),
            Self::IconAndText => fl!("message-actions-icon-and-text"),
        }
    }
}
