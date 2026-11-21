// SPDX-License-Identifier: GPL-3.0-only

use cosmic::iced::Length;
use cosmic::{Element, widget};
use uuid::Uuid;

use crate::fl;

use super::{Accounts, Field, Form, Message, Password, Screen, Status};

impl Accounts {
    pub fn button(&self) -> Element<'_, Message> {
        widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
            .on_press(Message::Open)
            .tooltip(fl!("accounts"))
            .into()
    }

    pub fn dialog(&self) -> Option<Element<'_, Message>> {
        let view = match &self.screen {
            Screen::Closed => return None,
            Screen::List => self.list(),
            Screen::Edit(form) => self.edit(form),
            Screen::ConfirmRemove(id) => self.confirm(*id),
        };
        Some(view)
    }

    fn list(&self) -> Element<'_, Message> {
        let mut rows = widget::column::with_capacity(self.accounts.len() + 2).spacing(10);
        if self.accounts.is_empty() && self.store.is_ok() {
            rows = rows.push(widget::text::body(fl!("account-empty")));
        }
        for account in &self.accounts {
            rows = rows.push(
                widget::row::with_capacity(3)
                    .push(widget::text::body(account.email.as_str()).width(Length::Fill))
                    .push(
                        widget::button::text(fl!("account-edit"))
                            .on_press(Message::Edit(account.id)),
                    )
                    .push(
                        widget::button::text(fl!("account-remove"))
                            .on_press(Message::ConfirmRemove(account.id)),
                    )
                    .spacing(4)
                    .align_y(cosmic::iced::Alignment::Center),
            );
        }

        widget::dialog()
            .title(fl!("accounts"))
            .control(
                widget::container(
                    widget::scrollable(rows.push(self.status_text())).height(Length::Shrink),
                )
                .max_height(320),
            )
            .primary_action(
                widget::button::suggested(fl!("account-add"))
                    .on_press_maybe(self.store.is_ok().then_some(Message::Add)),
            )
            .secondary_action(
                widget::button::standard(fl!("account-close")).on_press(Message::Close),
            )
            .into()
    }

    fn edit<'a>(&'a self, form: &'a Form) -> Element<'a, Message> {
        let editable = !self.is_busy();
        let fields = widget::column::with_capacity(8)
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
                    if form.id.is_some() {
                        fl!("account-password-keep")
                    } else {
                        fl!("account-password")
                    },
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
                widget::toggler(form.smtp_starttls)
                    .label(fl!("account-starttls"))
                    .on_toggle_maybe(editable.then_some(Message::StartTlsChanged)),
            )
            .push(self.status_text())
            .spacing(10);
        widget::dialog()
            .title(if form.id.is_some() {
                fl!("account-edit")
            } else {
                fl!("account-add")
            })
            .control(
                widget::container(widget::scrollable(fields).height(Length::Shrink))
                    .max_height(410),
            )
            .primary_action(
                widget::button::suggested(fl!("account-save"))
                    .on_press_maybe(editable.then_some(Message::Save)),
            )
            .secondary_action(
                widget::button::standard(fl!("account-back"))
                    .on_press_maybe(editable.then_some(Message::Back)),
            )
            .tertiary_action(
                widget::button::text(fl!("account-check"))
                    .on_press_maybe(editable.then_some(Message::Check)),
            )
            .into()
    }

    fn confirm(&self, id: Uuid) -> Element<'_, Message> {
        let email = self
            .accounts
            .iter()
            .find(|account| account.id == id)
            .map_or("", |account| account.email.as_str());
        let editable = !self.is_busy();
        widget::dialog()
            .title(fl!("account-remove"))
            .body(fl!("account-remove-confirm", email = email))
            .control(self.status_text())
            .primary_action(
                widget::button::destructive(fl!("account-remove"))
                    .on_press_maybe(editable.then_some(Message::Remove(id))),
            )
            .secondary_action(
                widget::button::standard(fl!("account-back"))
                    .on_press_maybe(editable.then_some(Message::Back)),
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
