// SPDX-License-Identifier: GPL-3.0-only

use cosmic::iced::Length;
use cosmic::{Element, widget};
use uuid::Uuid;

use crate::fl;

use super::{Accounts, Field, Form, Message, Password, Screen, Status};

impl Accounts {
    pub fn view(&self) -> Element<'_, Message> {
        match &self.screen {
            Screen::List => self.list(),
            Screen::Edit(form) => self.edit(form),
            Screen::ConfirmRemove(id) => self.confirm(*id),
        }
    }

    fn list(&self) -> Element<'_, Message> {
        let mut rows =
            widget::column::with_capacity(self.accounts.len() + self.available.len() + 2)
                .spacing(10);
        let editable = !self.is_busy();
        for account in &self.accounts {
            let mut row = widget::row::with_capacity(3)
                .push(widget::text::body(account.email.as_str()).width(Length::Fill));
            if account.online {
                let available = self.available.iter().any(|online| online.id == account.id);
                row = row.push(
                    widget::toggler(true).on_toggle_maybe(
                        (editable && available && self.store.is_ok())
                            .then_some(move |_| Message::ToggleLink(account.id, false)),
                    ),
                );
            } else {
                row = row
                    .push(
                        widget::button::icon(widget::icon::from_name("edit-symbolic"))
                            .tooltip(fl!("account-edit"))
                            .on_press_maybe(editable.then_some(Message::Edit(account.id))),
                    )
                    .push(
                        widget::button::icon(widget::icon::from_name("user-trash-symbolic"))
                            .tooltip(fl!("account-remove"))
                            .on_press_maybe(editable.then_some(Message::ConfirmRemove(account.id))),
                    );
            }
            rows = rows.push(row.spacing(4).align_y(cosmic::iced::Alignment::Center));
        }

        let mut has_available = false;
        for account in &self.available {
            if self.accounts.iter().any(|saved| saved.id == account.id) {
                continue;
            }
            has_available = true;
            rows = rows.push(
                widget::row::with_children([
                    widget::text::body(account.email.as_str())
                        .width(Length::Fill)
                        .into(),
                    widget::toggler(false)
                        .on_toggle_maybe(
                            (editable && self.store.is_ok())
                                .then_some(move |enabled| Message::ToggleLink(account.id, enabled)),
                        )
                        .into(),
                ])
                .align_y(cosmic::iced::Alignment::Center)
                .spacing(4),
            );
        }
        if self.accounts.is_empty()
            && !has_available
            && self.store.is_ok()
            && self.discovery_error.is_none()
        {
            let text = if self.discovering {
                fl!("account-loading")
            } else {
                fl!("account-empty")
            };
            rows = rows.push(widget::text::body(text));
        }
        if let Some(error) = self.discovery_error {
            rows = rows.push(widget::text::body(error.text()));
        }

        widget::settings::section()
            .title(fl!("accounts"))
            .add(rows)
            .add(
                widget::button::suggested(fl!("account-add"))
                    .on_press_maybe(editable.then_some(Message::Add)),
            )
            .into()
    }

    fn fields<'a>(&'a self, form: &'a Form) -> Element<'a, Message> {
        let editable = !self.is_busy();
        widget::column::with_capacity(8)
            .push(field(
                fl!("account-email"),
                &form.email,
                Field::Email,
                editable,
            ))
            .push(field(
                fl!("account-username"),
                &form.username,
                Field::Username,
                editable,
            ))
            .push(
                widget::secure_input(
                    fl!("account-password-keep"),
                    form.password.0.as_str(),
                    None,
                    true,
                )
                .editing(editable)
                .on_input(|value| Message::PasswordChanged(Password(value))),
            )
            .push(widget::text::heading(fl!("account-imap")))
            .push(server_fields(
                &form.imap_host,
                Field::ImapHost,
                &form.imap_port,
                Field::ImapPort,
                editable,
            ))
            .push(widget::text::heading(fl!("account-smtp")))
            .push(server_fields(
                &form.smtp_host,
                Field::SmtpHost,
                &form.smtp_port,
                Field::SmtpPort,
                editable,
            ))
            .push(
                widget::settings::item::builder(fl!("account-starttls")).control(
                    widget::toggler(form.smtp_starttls)
                        .on_toggle_maybe(editable.then_some(Message::StartTlsChanged)),
                ),
            )
            .push(self.status_text())
            .spacing(10)
            .into()
    }

    fn edit<'a>(&'a self, form: &'a Form) -> Element<'a, Message> {
        let editable = !self.is_busy();
        widget::settings::section()
            .title(fl!("account-edit"))
            .add(self.fields(form))
            .add(
                widget::row::with_children([
                    widget::button::standard(fl!("account-back"))
                        .on_press_maybe(editable.then_some(Message::Back))
                        .into(),
                    widget::button::text(fl!("account-check"))
                        .on_press_maybe(editable.then_some(Message::Check))
                        .into(),
                    widget::button::suggested(fl!("account-save"))
                        .on_press_maybe(editable.then_some(Message::Save))
                        .into(),
                ])
                .spacing(8)
                .wrap(),
            )
            .into()
    }

    fn confirm(&self, id: Uuid) -> Element<'_, Message> {
        let account = self.accounts.iter().find(|account| account.id == id);
        let email = account.map_or("", |account| account.email.as_str());
        let title = fl!("account-remove");
        let confirmation = fl!("account-remove-confirm", email = email);
        let editable = !self.is_busy();
        widget::settings::section()
            .title(title.clone())
            .add(widget::text::body(confirmation))
            .add(self.status_text())
            .add(
                widget::row::with_children([
                    widget::button::standard(fl!("account-back"))
                        .on_press_maybe(editable.then_some(Message::Back))
                        .into(),
                    widget::button::destructive(title)
                        .on_press_maybe(editable.then_some(Message::Remove(id)))
                        .into(),
                ])
                .spacing(8),
            )
            .into()
    }

    fn status_text(&self) -> Element<'_, Message> {
        let status = self
            .status
            .or_else(|| self.store.as_ref().err().copied().map(Status::Error));
        match status {
            Some(status) => widget::text::body(status.text()).into(),
            None => widget::Space::new().height(0).into(),
        }
    }
}

fn field(label: String, value: &str, field: Field, editable: bool) -> Element<'_, Message> {
    widget::text_input(label, value)
        .editing(editable)
        .on_input(move |value| Message::Change(field, value))
        .into()
}

fn server_fields<'a>(
    host: &'a str,
    host_field: Field,
    port: &'a str,
    port_field: Field,
    editable: bool,
) -> Element<'a, Message> {
    widget::row::with_capacity(2)
        .push(
            widget::container(field(fl!("account-server"), host, host_field, editable))
                .width(Length::Fill),
        )
        .push(widget::container(field(fl!("account-port"), port, port_field, editable)).width(90))
        .spacing(8)
        .into()
}
