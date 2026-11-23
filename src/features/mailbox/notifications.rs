// SPDX-License-Identifier: GPL-3.0-only

use crate::fl;
use std::collections::HashMap;
use zbus::zvariant::Value;

const APP_ID: &str = "dev.edfloreshz.Mail";

pub(super) async fn new_mail(account: uuid::Uuid, count: usize) {
    if let Err(error) = send(account, count).await {
        eprintln!("Could not show new-mail notification: {error}");
    }
}

async fn send(account: uuid::Uuid, count: usize) -> zbus::Result<()> {
    let bus = zbus::Connection::session().await?;
    let title = fl!("mail-new-notification");
    let body = fl!("mail-new-notification-count", count = count);
    if std::path::Path::new("/.flatpak-info").is_file() {
        let portal = zbus::Proxy::new(
            &bus,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Notification",
        )
        .await?;
        let notification = HashMap::from([
            ("title", Value::from(title.as_str())),
            ("body", Value::from(body.as_str())),
        ]);
        portal
            .call(
                "AddNotification",
                &(format!("new-mail-{account}"), notification),
            )
            .await
    } else {
        let daemon = zbus::Proxy::new(
            &bus,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .await?;
        let hints = HashMap::from([("desktop-entry", Value::from(APP_ID))]);
        daemon
            .call::<_, _, u32>(
                "Notify",
                &(
                    fl!("app-title"),
                    0_u32,
                    APP_ID,
                    title,
                    body,
                    Vec::<String>::new(),
                    hints,
                    -1_i32,
                ),
            )
            .await?;
        Ok(())
    }
}
