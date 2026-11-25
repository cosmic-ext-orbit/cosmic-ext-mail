// SPDX-License-Identifier: GPL-3.0-only

use std::borrow::Cow;

use accounts::{
    Provider, Service,
    ui::{Labels, View as AccountsView, provider_label},
};
use cosmic::iced::Length;
use cosmic::{Element, widget};
use uuid::Uuid;

use crate::fl;

use super::{Accounts, Field, Form, Message, Password, Screen, Status};

/// Mail keeps its app-local IMAP/SMTP editor, so the embedded panel only
/// offers browser-based sign-in providers.
const OAUTH_PROVIDERS: &[Provider] = &[Provider::Google, Provider::Microsoft];

fn waiting_label(provider: Provider) -> String {
    fl!("accounts-waiting", provider = provider_label(provider))
}

fn sign_in_label(provider: Provider) -> String {
    fl!("accounts-sign-in", provider = provider_label(provider))
}

fn add_provider(provider: Provider) -> Message {
    Message::AddProvider(provider)
}

fn cancel_auth(state: &str) -> Message {
    Message::CancelAuth(state.to_owned())
}

fn reconnect(id: Uuid) -> Message {
    Message::Reconnect(id)
}

fn remove_online(id: Uuid) -> Message {
    Message::RemoveOnline(id)
}

fn toggle_link(id: Uuid, enabled: bool) -> Message {
    Message::ToggleLink(id, enabled)
}

impl Accounts {
    pub fn view(&self) -> Element<'_, Message> {
        match &self.screen {
            Screen::List => self.list(),
            Screen::Edit(form) => self.edit(form),
            Screen::ConfirmRemove(id) => self.confirm(*id),
        }
    }

    fn list(&self) -> Element<'_, Message> {
        widget::column::with_capacity(2)
            .push(self.online())
            .push(self.local())
            .into()
    }

    fn online(&self) -> Element<'_, Message> {
        let error: Option<Cow<'_, str>> = self.discovery_error.map(|error| error.text().into());
        let labels = Labels {
            title: fl!("online-accounts").into(),
            empty: fl!("account-empty").into(),
            waiting: &waiting_label,
            waiting_description: fl!("accounts-waiting-description").into(),
            cancel: fl!("account-cancel").into(),
            reconnect: fl!("accounts-reconnect").into(),
            remove: fl!("accounts-remove").into(),
            add: fl!("account-add").into(),
            add_description: fl!("accounts-add-description").into(),
            sign_in: &sign_in_label,
            ..Labels::default()
        };

        widget::column::with_capacity(2)
            .push(accounts::ui::view(AccountsView {
                service: Service::Mail,
                accounts: &self.managed,
                pending: self.auth(),
                error,
                providers: Some(OAUTH_PROVIDERS),
                labels,
                on_add: &add_provider,
                on_cancel: &cancel_auth,
                on_remove: &remove_online,
                on_reconnect: &reconnect,
                on_toggle: &toggle_link,
            }))
            .push(self.status_text())
            .into()
    }

    fn local(&self) -> Element<'_, Message> {
        let mut section = widget::settings::section().title(fl!("accounts"));
        let editable = !self.is_busy();
        for account in &self.accounts {
            if account.online {
                continue;
            }
            let item = widget::settings::item::builder(account.email.as_str());
            section = section.add(item.control(
                widget::row::with_capacity(2)
                    .push(
                        widget::button::icon(widget::icon::from_name("edit-symbolic"))
                            .tooltip(fl!("account-edit"))
                            .on_press_maybe(editable.then_some(Message::Edit(account.id))),
                    )
                    .push(
                        widget::button::icon(widget::icon::from_name("user-trash-symbolic"))
                            .tooltip(fl!("account-remove"))
                            .on_press_maybe(editable.then_some(Message::ConfirmRemove(account.id))),
                    )
                    .spacing(4),
            ));
        }

        section
            .add(
                widget::button::standard(fl!("account-add-local"))
                    .on_press_maybe(editable.then_some(Message::AddLocal)),
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
