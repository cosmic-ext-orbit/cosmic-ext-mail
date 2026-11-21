// SPDX-License-Identifier: GPL-3.0-only

use crate::{
    features::mailbox::{Folder, Mail},
    fl,
};
use cosmic::iced::{Length, Subscription};
use cosmic::{Element, widget};
use cosmic_ext_mail::EmailView;

#[derive(Clone, Debug)]
pub enum ReaderMessage {
    Wake,
    BackToList,
    ToggleUnread,
    MoveTo(Folder),
    Reply,
}

pub struct Reader {
    email: Result<EmailView, String>,
    body_error: Option<String>,
    open: bool,
}

impl Reader {
    pub fn new(mail: Option<&Mail>) -> Self {
        let body = mail.map_or(cosmic_ext_mail::EmailBody::PlainText(""), |mail| mail.body);
        Self {
            email: EmailView::new(body),
            body_error: None,
            open: false,
        }
    }

    pub fn set_message(&mut self, mail: Option<&Mail>) {
        self.body_error = match (&mut self.email, mail) {
            (Ok(view), Some(mail)) => view.set_body(mail.body).err(),
            _ => None,
        };
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub fn subscription(&self) -> Subscription<ReaderMessage> {
        self.email.as_ref().map_or_else(
            |_| Subscription::none(),
            |view| view.subscription().map(|()| ReaderMessage::Wake),
        )
    }

    pub fn view<'a>(&'a self, mail: Option<&'a Mail>, narrow: bool) -> Element<'a, ReaderMessage> {
        let mut page = widget::column::with_capacity(5)
            .spacing(8)
            .padding(12)
            .height(Length::Fill);
        if narrow {
            page = page.push(
                widget::button::text(fl!("back-to-list")).on_press(ReaderMessage::BackToList),
            );
        }

        let Some(mail) = mail else {
            return page.push(widget::text::body(fl!("select-message"))).into();
        };
        let unread_action = if mail.unread {
            fl!("mark-read")
        } else {
            fl!("mark-unread")
        };
        let actions = widget::row::with_capacity(4)
            .push(widget::button::standard(fl!("reply")).on_press(ReaderMessage::Reply))
            .push(widget::button::text(fl!("archive-action")).on_press_maybe(
                (mail.folder != Folder::Archive).then_some(ReaderMessage::MoveTo(Folder::Archive)),
            ))
            .push(widget::button::text(fl!("trash-action")).on_press_maybe(
                (mail.folder != Folder::Trash).then_some(ReaderMessage::MoveTo(Folder::Trash)),
            ))
            .push(widget::button::text(unread_action).on_press(ReaderMessage::ToggleUnread))
            .spacing(4)
            .wrap();
        let body: Element<_> = match (&self.email, &self.body_error) {
            (Ok(view), None) => view.view(),
            (Err(error), _) | (_, Some(error)) => {
                widget::text(fl!("preview-failed", error = error.as_str())).into()
            }
        };
        page = page
            .push(actions)
            .push(widget::text::title2(mail.subject))
            .push(widget::text::body(format!(
                "{} <{}>",
                mail.sender, mail.address
            )))
            .push(widget::text::caption(mail.time));
        if let Some(filename) = mail.attachment {
            page = page.push(widget::text::caption(fl!(
                "attachment",
                filename = filename
            )));
        }
        page.push(body).into()
    }
}
