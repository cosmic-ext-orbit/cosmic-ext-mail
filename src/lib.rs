// SPDX-License-Identifier: GPL-3.0-only

#[path = "features/email/mod.rs"]
mod email;

pub use email::{EmailBody, EmailView, PreparedEmail};
