// SPDX-License-Identifier: GPL-3.0-only

use crate::{features::mailbox::Mail, fl};
use cosmic::iced::widget::text_editor;
use cosmic::{Element, widget};

#[derive(Clone, Debug)]
pub enum ComposerMessage {
    Open,
    ToChanged(String),
    SubjectChanged(String),
    BodyChanged(text_editor::Action),
    Save,
}

#[derive(Default)]
struct Draft {
    to: String,
    subject: String,
    body: text_editor::Content<cosmic::Renderer>,
}

#[derive(Default)]
pub struct Composer {
    open: bool,
    draft: Option<Draft>,
}

impl Composer {
    pub fn new_message_button(&self) -> Element<'_, ComposerMessage> {
        widget::button::suggested(fl!("new-message"))
            .on_press(ComposerMessage::Open)
            .into()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    pub fn reply(&mut self, mail: Option<&Mail>) {
        if self.draft.is_none()
            && let Some(mail) = mail
        {
            self.draft = Some(Draft {
                to: mail.address.to_owned(),
                subject: format!("Re: {}", mail.subject),
                ..Draft::default()
            });
        }
        self.open = self.draft.is_some();
    }

    pub fn update(&mut self, message: ComposerMessage) {
        match message {
            ComposerMessage::Open => {
                self.draft.get_or_insert_with(Draft::default);
                self.open = true;
            }
            ComposerMessage::ToChanged(to) => {
                if let Some(draft) = &mut self.draft {
                    draft.to = to;
                }
            }
            ComposerMessage::SubjectChanged(subject) => {
                if let Some(draft) = &mut self.draft {
                    draft.subject = subject;
                }
            }
            ComposerMessage::BodyChanged(action) => {
                if let Some(draft) = &mut self.draft {
                    draft.body.perform(action);
                }
            }
            ComposerMessage::Save => self.open = false,
        }
    }

    pub fn dialog(&self) -> Option<Element<'_, ComposerMessage>> {
        let draft = self.draft.as_ref().filter(|_| self.open)?;
        let composer: Element<'_, ComposerMessage> = widget::dialog()
            .title(fl!("new-message"))
            .control(
                widget::column::with_capacity(3)
                    .push(
                        widget::text_input(fl!("recipient"), draft.to.as_str())
                            .on_input(ComposerMessage::ToChanged),
                    )
                    .push(
                        widget::text_input(fl!("subject"), draft.subject.as_str())
                            .on_input(ComposerMessage::SubjectChanged),
                    )
                    .push(
                        widget::TextEditor::new(&draft.body)
                            .height(150)
                            .on_action(ComposerMessage::BodyChanged),
                    )
                    .spacing(8),
            )
            .primary_action(
                widget::button::suggested(fl!("save-draft")).on_press(ComposerMessage::Save),
            )
            .into();
        Some(composer)
    }
}
