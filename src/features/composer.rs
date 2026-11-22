// SPDX-License-Identifier: GPL-3.0-only

use crate::{features::mailbox::Mail, fl};
use cosmic::iced::widget::text_editor;
use cosmic::{Element, widget};

#[derive(Clone, Debug)]
pub enum Message {
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

    pub fn forward(&mut self, mail: &Mail, text: &str) -> bool {
        if self.draft.is_some() {
            return false;
        }
        let quoted = fl!(
            "forwarded-message",
            sender = mail.sender.as_str(),
            subject = mail.subject.as_str(),
            date = mail.time.as_str(),
            body = text,
        );
        self.draft = Some(Draft {
            subject: fl!("forward-subject", subject = mail.subject.as_str()),
            body: text_editor::Content::with_text(&quoted),
            ..Draft::default()
        });
        self.open = true;
        true
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Open => {
                self.draft.get_or_insert_with(Draft::default);
                self.open = true;
            }
            Message::ToChanged(to) => {
                if let Some(draft) = &mut self.draft {
                    draft.to = to;
                }
            }
            Message::SubjectChanged(subject) => {
                if let Some(draft) = &mut self.draft {
                    draft.subject = subject;
                }
            }
            Message::BodyChanged(action) => {
                if let Some(draft) = &mut self.draft {
                    draft.body.perform(action);
                }
            }
            Message::Save => self.open = false,
        }
    }

    pub fn new_message_button(&self) -> Element<'_, Message> {
        widget::button::icon(widget::icon::from_name("list-add-symbolic"))
            .tooltip(fl!("new-message"))
            .on_press(Message::Open)
            .into()
    }

    pub fn dialog(&self) -> Option<Element<'_, Message>> {
        let draft = self.draft.as_ref().filter(|_| self.open)?;
        let composer: Element<'_, Message> = widget::dialog()
            .title(fl!("new-message"))
            .control(
                widget::column::with_capacity(3)
                    .push(
                        widget::text_input(fl!("recipient"), draft.to.as_str())
                            .on_input(Message::ToChanged),
                    )
                    .push(
                        widget::text_input(fl!("subject"), draft.subject.as_str())
                            .on_input(Message::SubjectChanged),
                    )
                    .push(
                        widget::TextEditor::new(&draft.body)
                            .height(150)
                            .on_action(Message::BodyChanged),
                    )
                    .spacing(8),
            )
            .primary_action(widget::button::suggested(fl!("save-draft")).on_press(Message::Save))
            .into();
        Some(composer)
    }
}
