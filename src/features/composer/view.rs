// SPDX-License-Identifier: GPL-3.0-only

use super::{Composer, Delivery, Message, Session};
use crate::{features::accounts::Account, fl};
use cosmic::{
    Element, Theme,
    iced::{Alignment, Border, Length, widget::text_editor},
    widget,
};
use uuid::Uuid;

impl Composer {
    pub fn draft_count(&self) -> usize {
        self.drafts.len()
    }

    pub fn draft_rows(&self) -> Vec<(Uuid, String)> {
        self.drafts
            .iter()
            .map(|draft| {
                let label = if draft.subject.trim().is_empty() {
                    fl!("mail-no-subject")
                } else {
                    draft.subject.clone()
                };
                (draft.id, label)
            })
            .collect()
    }

    pub fn draft_read_error(&self) -> bool {
        self.storage_error
    }

    pub fn new_message_button(&self) -> Element<'_, Message> {
        widget::tooltip(
            widget::button::icon(widget::icon::from_name("list-add-symbolic"))
                .name(fl!("new-message"))
                .on_press(Message::Open),
            widget::text(fl!("new-message")),
            widget::tooltip::Position::Bottom,
        )
        .into()
    }

    pub fn dialog<'a>(&'a self, accounts: &'a [Account]) -> Option<Element<'a, Message>> {
        let session = self.active.as_ref()?;
        let draft = &session.draft;
        let editable = session.editable();
        let selected = accounts
            .iter()
            .position(|account| Some(account.id) == draft.account);
        let from: Element<_> = if accounts.is_empty() {
            widget::text::caption(fl!("compose-no-accounts")).into()
        } else if editable {
            widget::dropdown(
                accounts
                    .iter()
                    .map(|account| account.email.clone())
                    .collect::<Vec<_>>(),
                selected,
                Message::SetFrom,
            )
            .placeholder(fl!("compose-select-account"))
            .into()
        } else {
            widget::text::body(selected.map_or("", |index| accounts[index].email.as_str())).into()
        };

        let mut fields = widget::column::with_capacity(5)
            .spacing(6)
            .push(
                widget::row::with_capacity(4)
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(widget::text::body(fl!("compose-from")))
                    .push(from)
                    .push(widget::space::horizontal())
                    .push(
                        widget::button::text(if session.extra_recipients {
                            fl!("hide-cc-bcc")
                        } else {
                            fl!("show-cc-bcc")
                        })
                        .on_press_maybe(editable.then_some(Message::ToggleRecipients)),
                    ),
            )
            .push(address_field(session, 0, fl!("recipient")));
        if session.extra_recipients {
            fields = fields
                .push(address_field(session, 1, fl!("compose-cc")))
                .push(address_field(session, 2, fl!("compose-bcc")));
        }
        fields = fields.push(
            widget::text_input(fl!("subject"), draft.subject.as_str())
                .label(fl!("subject"))
                .editing(editable)
                .on_input(Message::SubjectChanged),
        );
        let mut editor = widget::TextEditor::new(&session.body)
            .height(Length::Fill)
            .padding(cosmic::theme::spacing().space_xxs)
            .style(compose_editor_style);
        if editable {
            editor = editor.on_action(Message::BodyChanged);
        }
        let mut page = widget::column::with_capacity(6)
            .padding(12)
            .spacing(8)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(
                widget::container(widget::scrollable(fields).height(Length::Shrink))
                    .max_height(225),
            )
            .push(editor);
        if !draft.attachments.is_empty() {
            let mut files = widget::column::with_capacity(draft.attachments.len()).spacing(4);
            for file in &draft.attachments {
                let size = fl!(
                    "attachment-size",
                    size = format!("{:.1}", file.size as f64 / 1024.0)
                );
                files = files.push(
                    widget::row::with_capacity(3)
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(widget::text::body(file.name.as_str()).width(Length::Fill))
                        .push(widget::text::caption(size))
                        .push(
                            widget::button::icon(widget::icon::from_name("user-trash-symbolic"))
                                .tooltip(fl!("remove-attachment"))
                                .on_press_maybe(
                                    editable.then_some(Message::RemoveAttachment(file.id)),
                                ),
                        ),
                );
            }
            page = page.push(
                widget::container(widget::scrollable(files).height(Length::Shrink)).max_height(110),
            );
        }
        if session.confirm_discard {
            page = page.push(widget::text::body(fl!("discard-draft-confirm")));
        }
        if session.sending {
            page = page.push(widget::text::body(fl!("message-sending")));
        } else if session.importing {
            page = page.push(widget::text::body(fl!("attachment-working")));
        } else if draft.delivery == Delivery::Uncertain {
            page = page.push(widget::text::body(fl!("smtp-uncertain")));
        }
        if let Some(status) = &session.status {
            page = page.push(widget::text::body(status.as_str()));
        } else if self.store.is_none() {
            page = page.push(widget::text::body(fl!("draft-storage-error")));
        }
        let dialog = widget::dialog()
            .title(fl!("new-message"))
            .width(Length::Fill)
            .height(Length::Fill)
            .max_width(800)
            .max_height(700)
            .control(page);
        if session.confirm_discard {
            return Some(
                dialog
                    .secondary_action(
                        widget::button::standard(fl!("compose-cancel"))
                            .on_press(Message::ConfirmDiscard(false)),
                    )
                    .primary_action(
                        widget::button::destructive(fl!("discard-draft"))
                            .on_press(Message::Discard),
                    )
                    .into(),
            );
        }

        let primary: Element<_> = if draft.delivery == Delivery::Uncertain {
            widget::button::suggested(fl!("smtp-retry"))
                .on_press_maybe(editable.then_some(Message::Retry))
                .into()
        } else {
            widget::button::suggested(fl!("send-message"))
                .leading_icon(widget::icon::from_name("mail-send-symbolic"))
                .on_press_maybe(
                    (editable
                        && draft.delivery == Delivery::Ready
                        && self.store.is_some()
                        && !accounts.is_empty())
                    .then_some(Message::Send),
                )
                .into()
        };
        Some(
            dialog
                .tertiary_action(
                    widget::row::with_capacity(2)
                        .spacing(4)
                        .push(
                            widget::button::icon(widget::icon::from_name(
                                "mail-attachment-symbolic",
                            ))
                            .tooltip(fl!("attach-files"))
                            .on_press_maybe(editable.then_some(Message::Attach)),
                        )
                        .push(
                            widget::button::icon(widget::icon::from_name("user-trash-symbolic"))
                                .tooltip(fl!("discard-draft"))
                                .on_press_maybe(
                                    (!session.busy() && draft.delivery != Delivery::Sent)
                                        .then_some(Message::ConfirmDiscard(true)),
                                ),
                        )
                        .wrap(),
                )
                .secondary_action(
                    widget::button::standard(fl!("save-and-close-draft"))
                        .on_press_maybe((!session.busy()).then_some(Message::Close)),
                )
                .primary_action(primary)
                .into(),
        )
    }
}

fn compose_editor_style(theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    use cosmic::{theme::TextInput, widget::text_input::StyleSheet};

    let appearance = match status {
        text_editor::Status::Active => theme.active(&TextInput::Default),
        text_editor::Status::Hovered => theme.hovered(&TextInput::Default),
        text_editor::Status::Focused { .. } => theme.focused(&TextInput::Default),
        text_editor::Status::Disabled => theme.disabled(&TextInput::Default),
    };

    text_editor::Style {
        background: appearance.background,
        border: Border {
            color: appearance.border_color,
            width: appearance.border_width,
            radius: appearance.border_radius,
        },
        value: appearance
            .text_color
            .unwrap_or_else(|| theme.current_container().on.into()),
        placeholder: appearance.placeholder_color,
        selection: appearance.selected_fill,
    }
}

fn address_field(session: &Session, field: usize, label: String) -> Element<'_, Message> {
    let editable = session.editable();
    let mut row = widget::row::with_capacity(session.draft.recipients[field].len() + 2)
        .spacing(4)
        .align_y(Alignment::Center)
        .push(widget::text::body(label.clone()));
    for (index, address) in session.draft.recipients[field].iter().enumerate() {
        row = row.push(
            widget::button::standard(address.as_str())
                .trailing_icon(widget::icon::from_name("window-close-symbolic"))
                .tooltip(fl!("remove-recipient"))
                .on_press_maybe(editable.then_some(Message::RemoveAddress(field, index))),
        );
    }
    let mut input = widget::text_input(
        fl!("compose-email-address"),
        session.draft.inputs[field].as_str(),
    )
    .editing(editable)
    .width(Length::Fill)
    .on_input(move |input| Message::AddressChanged(field, input))
    .on_submit(move |_| Message::AddAddress(field));
    if session.invalid_address == Some(field) {
        input = input.error(fl!("compose-invalid-address"));
    }
    widget::column::with_capacity(2)
        .spacing(4)
        .push(row.wrap())
        .push(
            widget::row::with_capacity(2)
                .spacing(6)
                .align_y(Alignment::Center)
                .push(input)
                .push(
                    widget::button::icon(widget::icon::from_name("list-add-symbolic"))
                        .tooltip(fl!("compose-add-recipient"))
                        .on_press_maybe(editable.then_some(Message::AddAddress(field))),
                ),
        )
        .into()
}
