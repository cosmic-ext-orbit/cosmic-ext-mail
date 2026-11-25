// SPDX-License-Identifier: GPL-3.0-only

use accounts::{Provider, Service, provider::providers_for, ui::provider_label};
use cosmic::iced::Alignment;
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
        widget::column::with_capacity(2)
            .push(self.online())
            .push(self.local())
            .into()
    }

    fn online(&self) -> Element<'_, Message> {
        let mut section = widget::settings::section().title(fl!("online-accounts"));
        let editable = !self.is_busy() && self.auth().is_none();

        if let Some(error) = self.discovery_error {
            section = section.add(widget::text::body(error.text()));
        } else if self.managed.is_empty() && self.auth().is_none() && !self.discovering {
            section = section.add(widget::text::body(fl!("account-empty")));
        }

        if let Some(request) = self.auth() {
            let state = request.state.clone();
            section = section.add(
                widget::settings::item::builder(fl!(
                    "accounts-waiting",
                    provider = provider_label(request.provider)
                ))
                .description(fl!("accounts-waiting-description"))
                .control(
                    widget::button::standard(fl!("account-cancel"))
                        .on_press(Message::CancelAuth(state)),
                ),
            );
        }

        for account in &self.managed {
            let id = account.id;
            let enabled = self
                .usable_online
                .as_ref()
                .map_or(account.enabled, |ids| ids.contains(&id));
            let controls = widget::row::with_capacity(3)
                .spacing(cosmic::theme::spacing().space_xs)
                .align_y(Alignment::Center)
                .push(
                    widget::button::standard(fl!("accounts-reconnect"))
                        .on_press_maybe(editable.then_some(Message::Reconnect(id))),
                )
                .push(
                    widget::button::destructive(fl!("accounts-remove"))
                        .on_press_maybe(editable.then_some(Message::RemoveOnline(id))),
                )
                .push(
                    widget::toggler(enabled)
                        .on_toggle_maybe(editable.then_some(move |enabled| {
                            Message::ToggleLink(id, enabled)
                        })),
                );
            section = section.add(
                widget::settings::item::builder(account.email_or_identity())
                    .description(provider_label(account.provider))
                    .control(controls),
            );
        }

        let mut providers = widget::row::with_capacity(2)
            .spacing(cosmic::theme::spacing().space_xs);
        for provider in providers_for(Service::Mail)
            .into_iter()
            .filter(|provider| matches!(provider, Provider::Google | Provider::Microsoft))
        {
            providers = providers.push(
                widget::button::standard(fl!(
                    "accounts-sign-in",
                    provider = provider_label(provider)
                ))
                .on_press_maybe(editable.then_some(Message::AddProvider(provider))),
            );
        }
        section = section.add(
            widget::settings::item::builder(fl!("account-add"))
                .description(fl!("accounts-add-description"))
                .control(providers),
        );

        section.add(self.status_text()).into()
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
