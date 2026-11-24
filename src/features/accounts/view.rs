// SPDX-License-Identifier: GPL-3.0-only

use cosmic::iced::Length;
use cosmic::{Element, widget};
use uuid::Uuid;

use crate::fl;

use super::{Account, Accounts, Field, Form, Message, Password, Screen, Status};

impl Accounts {
    pub fn view(&self) -> Element<'_, Message> {
        match &self.screen {
            Screen::List => self.list(),
            Screen::Online(available) => self.online_accounts(available),
            Screen::Edit(form) if form.id.is_some() => self.edit(form),
            Screen::Edit(_) => self.list(),
            Screen::ConfirmRemove(id) => self.confirm(*id),
        }
    }

    pub fn add_dialog(&self) -> Option<Element<'_, Message>> {
        let Screen::Edit(form) = &self.screen else {
            return None;
        };
        if form.id.is_some() {
            return None;
        }
        let editable = !self.is_busy();
        Some(
            widget::dialog()
                .title(fl!("account-add"))
                .width(Length::Fill)
                .max_width(520)
                .control(
                    widget::column::with_capacity(2)
                        .push(
                            widget::button::text(fl!("account-use-online"))
                                .on_press_maybe(editable.then_some(Message::DiscoverOnline)),
                        )
                        .push(widget::scrollable(self.fields(form)).height(Length::Shrink))
                        .spacing(8),
                )
                .tertiary_action(
                    widget::button::text(fl!("account-check"))
                        .on_press_maybe(editable.then_some(Message::Check)),
                )
                .secondary_action(
                    widget::button::standard(fl!("account-cancel"))
                        .on_press_maybe(editable.then_some(Message::Back)),
                )
                .primary_action(
                    widget::button::suggested(fl!("account-save"))
                        .on_press_maybe(editable.then_some(Message::Save)),
                )
                .into(),
        )
    }

    fn list(&self) -> Element<'_, Message> {
        let mut rows = widget::column::with_capacity(self.accounts.len() + 2).spacing(10);
        if self.accounts.is_empty() && self.store.is_ok() {
            rows = rows.push(widget::text::body(fl!("account-empty")));
        }
        for account in &self.accounts {
            let row = widget::row::with_capacity(3)
                .push(widget::text::body(account.email.as_str()).width(Length::Fill));
            let row = if account.online {
                row.push(widget::text::caption(fl!("account-online-source")))
            } else {
                row.push(
                    widget::button::icon(widget::icon::from_name("edit-symbolic"))
                        .tooltip(fl!("account-edit"))
                        .on_press(Message::Edit(account.id)),
                )
            };
            rows = rows.push(
                row.push(
                    widget::button::icon(widget::icon::from_name("user-trash-symbolic"))
                        .tooltip(if account.online {
                            fl!("account-unlink")
                        } else {
                            fl!("account-remove")
                        })
                        .on_press(Message::ConfirmRemove(account.id)),
                )
                .spacing(4)
                .align_y(cosmic::iced::Alignment::Center),
            );
        }

        widget::settings::section()
            .title(fl!("accounts"))
            .add(rows.push(self.status_text()))
            .add(
                widget::row::with_children([
                    widget::button::suggested(fl!("account-add"))
                        .on_press_maybe(self.store.is_ok().then_some(Message::Add))
                        .into(),
                    widget::button::text(fl!("account-use-online"))
                        .on_press_maybe(self.store.is_ok().then_some(Message::DiscoverOnline))
                        .into(),
                ])
                .spacing(8)
                .wrap(),
            )
            .into()
    }

    fn online_accounts<'a>(&'a self, available: &'a [Account]) -> Element<'a, Message> {
        let mut section = widget::settings::section().title(fl!("account-online-source"));
        let mut any = false;
        for account in available {
            if self.accounts.iter().any(|saved| saved.id == account.id) {
                continue;
            }
            any = true;
            section = section.add(
                widget::settings::item::builder(account.email.as_str()).control(
                    widget::button::standard(fl!("account-link")).on_press_maybe(
                        (!self.is_busy()).then_some(Message::LinkOnline(account.id)),
                    ),
                ),
            );
        }
        if !any && self.status.is_none() {
            section = section.add(widget::text::body(fl!("account-online-empty")));
        }
        section
            .add(self.status_text())
            .add(
                widget::row::with_children([
                    widget::button::standard(fl!("account-back"))
                        .on_press_maybe((!self.is_busy()).then_some(Message::Back))
                        .into(),
                    widget::button::text(fl!("account-online-refresh"))
                        .on_press_maybe((!self.is_busy()).then_some(Message::DiscoverOnline))
                        .into(),
                ])
                .spacing(8)
                .wrap(),
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
        let online = account.is_some_and(|account| account.online);
        let title = if online {
            fl!("account-unlink")
        } else {
            fl!("account-remove")
        };
        let confirmation = if online {
            fl!("account-unlink-confirm", email = email)
        } else {
            fl!("account-remove-confirm", email = email)
        };
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
