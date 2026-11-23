// SPDX-License-Identifier: GPL-3.0-only

use crate::{
    config::{Config, MessageActions},
    features::mailbox::{Contact, Folder, Mail, MailId, remote},
    fl,
};
use cosmic::widget::menu;
use cosmic::{Element, widget};
use cosmic::{
    Task,
    dialog::file_chooser,
    iced::{Length, Subscription},
};
use cosmic_ext_mail::{EmailBody, EmailView};
use mail_parser::MessageParser;
use std::{collections::HashMap, sync::Arc};

const INLINE_ACTIONS_MIN_WIDTH: f32 = 520.0;

#[derive(Clone, Debug)]
pub enum Message {
    Wake,
    BackToList,
    Retry,
    ShowRemoteImages,
    SaveAttachment(u64, usize),
    AttachmentSaved(Result<(), ()>),
    SetTheme(MailId, Option<bool>),
    Loaded(u64, Result<Arc<[u8]>, remote::Error>),
    Reply,
    Forward,
    CopyAddress(String),
    ComposeTo(String),
    Change(remote::Change),
}

#[derive(Clone, Debug, Default)]
pub struct ComposeDetails {
    pub reply_address: Option<String>,
    pub message_id: Option<String>,
    pub text: String,
}

impl ComposeDetails {
    pub fn parse(bytes: &[u8]) -> Self {
        Self::from_message(MessageParser::new().parse(bytes).as_ref())
    }

    fn from_message(message: Option<&mail_parser::Message<'_>>) -> Self {
        Self {
            reply_address: message
                .and_then(|message| message.reply_to())
                .and_then(|addresses| addresses.first())
                .and_then(|mailbox| mailbox.address())
                .map(str::to_owned),
            message_id: message
                .and_then(|message| message.message_id())
                .map(str::to_owned),
            text: message
                .and_then(|message| message.body_text(0))
                .map_or_else(String::new, std::borrow::Cow::into_owned),
        }
    }
}

struct Attachment {
    name: String,
    size: usize,
    part: usize,
}

impl Attachment {
    fn list(message: &mail_parser::Message<'_>) -> Vec<Self> {
        message
            .attachments()
            .enumerate()
            .filter(|(_, part)| !part.is_inline())
            .map(|(index, part)| {
                let name = part
                    .attachment_name()
                    .and_then(|name| name.rsplit(['/', '\\']).next())
                    .map(|name| {
                        name.chars()
                            .filter(|ch| !ch.is_control())
                            .collect::<String>()
                    })
                    .and_then(|name| {
                        let name = name.trim();
                        (!matches!(name, "" | "." | "..")).then(|| name.to_owned())
                    })
                    .unwrap_or_else(|| fl!("mail-attachment-name", number = (index + 1)));
                Self {
                    name,
                    size: part.decoded_len(),
                    part: index,
                }
            })
            .collect()
    }
}

pub struct Reader {
    email: Option<EmailView>,
    theme_overrides: HashMap<MailId, bool>,
    body: Body,
    request: u64,
    recipients: Vec<Contact>,
    attachments: Vec<Attachment>,
    attachment_source: Option<Arc<[u8]>>,
    forward_text: String,
    reply_address: Option<String>,
    message_id: Option<String>,
    open: bool,
}

enum Body {
    Empty,
    Loading,
    Ready,
    Failed(String),
}

impl Reader {
    pub fn new() -> Self {
        Self {
            email: None,
            theme_overrides: HashMap::new(),
            body: Body::Empty,
            request: 0,
            recipients: Vec::new(),
            attachments: Vec::new(),
            attachment_source: None,
            forward_text: String::new(),
            reply_address: None,
            message_id: None,
            open: false,
        }
    }

    pub fn release_window(&mut self) {
        self.request += 1;
        self.email = None;
        self.attachments.clear();
        self.attachment_source = None;
        self.body = Body::Empty;
    }

    pub fn load(
        &mut self,
        mail: Option<&Mail>,
        accounts: &crate::features::accounts::Accounts,
    ) -> Task<Message> {
        self.request += 1;
        self.body = Body::Empty;
        self.recipients.clear();
        self.attachments.clear();
        self.attachment_source = None;
        self.forward_text.clear();
        self.reply_address = None;
        self.message_id = None;
        let Some(mail) = mail else {
            return Task::none();
        };
        let Some(account) = accounts.saved().ok().and_then(|accounts| {
            accounts
                .iter()
                .find(|account| account.id == mail.id.account)
        }) else {
            self.body = Body::Failed(remote::Error::MessageUnavailable.text());
            return Task::none();
        };
        self.body = Body::Loading;
        let request = self.request;
        Task::perform(
            remote::body(account.clone(), mail.id.clone()),
            move |result| Message::Loaded(request, result),
        )
    }

    pub fn finish_load(
        &mut self,
        request: u64,
        result: Result<Arc<[u8]>, remote::Error>,
        load_remote_images: bool,
    ) {
        if request != self.request {
            return;
        }
        let rendered = result.map_err(remote::Error::text).and_then(|bytes| {
            let message = MessageParser::new().parse(&bytes);
            let details = ComposeDetails::from_message(message.as_ref());
            self.reply_address = details.reply_address;
            self.message_id = details.message_id;
            self.forward_text = details.text;
            if let Some(addresses) = message.as_ref().and_then(|message| message.to()) {
                self.recipients = Contact::from_list(addresses);
            }

            if let Some(message) = message.as_ref() {
                self.attachments = Attachment::list(message);
            }
            if !self.attachments.is_empty() {
                self.attachment_source = Some(Arc::clone(&bytes));
            }
            let body = EmailBody::Mime(&bytes);
            if let Some(view) = &mut self.email {
                view.set_body_with_remote_images(body, load_remote_images)?;
            } else {
                self.email = Some(EmailView::new_with_appearance(
                    body,
                    load_remote_images,
                    cosmic::theme::is_dark(),
                )?);
            }
            Ok(())
        });
        self.body = match rendered {
            Ok(()) => Body::Ready,
            Err(error) => Body::Failed(error),
        };
    }

    pub fn save_attachment(&self, request: u64, index: usize) -> Task<Message> {
        if request != self.request {
            return Task::none();
        }
        let (Some(file), Some(source)) =
            (self.attachments.get(index), self.attachment_source.as_ref())
        else {
            return Task::none();
        };
        let name = file.name.clone();
        let part = file.part;
        let source = Arc::clone(source);
        Task::perform(
            async move {
                let destination = match file_chooser::save::Dialog::new()
                    .title(fl!("mail-save-attachment"))
                    .file_name(name)
                    .save_file()
                    .await
                {
                    Ok(response) => {
                        let Some(url) = response.url() else {
                            return Ok(());
                        };
                        url.to_file_path().map_err(|_| ())?
                    }
                    Err(file_chooser::Error::Cancelled) => return Ok(()),
                    Err(_) => return Err(()),
                };
                tokio::task::spawn_blocking(move || {
                    let message = MessageParser::new().parse(&source).ok_or(())?;
                    let attachment = message.attachments().nth(part).ok_or(())?;
                    std::fs::write(destination, attachment.decoded()).map_err(|_| ())
                })
                .await
                .map_err(|_| ())?
            },
            Message::AttachmentSaved,
        )
    }

    pub fn set_remote_images(&mut self, enabled: bool) {
        if matches!(self.body, Body::Ready)
            && let Some(view) = &mut self.email
        {
            view.set_remote_images(enabled);
        }
    }

    pub fn set_theme(&mut self, id: MailId, dark: Option<bool>) {
        if let Some(dark) = dark {
            self.theme_overrides.insert(id, dark);
        } else {
            self.theme_overrides.remove(&id);
        }
    }

    pub fn reply_address<'a>(&'a self, mail: &'a Mail) -> &'a str {
        self.reply_address.as_deref().unwrap_or(&mail.address)
    }

    pub fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    pub fn forward_text(&self) -> &str {
        &self.forward_text
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.body, Body::Loading)
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    pub fn subscription(&self) -> Subscription<Message> {
        self.email.as_ref().map_or_else(Subscription::none, |view| {
            view.subscription().map(|()| Message::Wake)
        })
    }

    pub fn view<'a>(
        &'a self,
        mail: Option<&'a Mail>,
        folder: Folder,
        narrow: bool,
        config: &'a Config,
        photos: Option<&'a crate::features::avatars::Avatars>,
    ) -> Element<'a, Message> {
        let mut page = widget::column::with_capacity(4).spacing(8).padding(12);
        if narrow {
            page =
                page.push(widget::button::text(fl!("back-to-list")).on_press(Message::BackToList));
        }

        let Some(mail) = mail else {
            return page.push(widget::text::body(fl!("select-message"))).into();
        };
        let header = page.push(
            widget::responsive(move |size| {
                self.header(
                    mail,
                    folder,
                    config,
                    photos.and_then(|photos| photos.get(&mail.address)),
                    size.width,
                )
            })
            .height(Length::Shrink),
        );
        let body: Element<_> = match &self.body {
            Body::Ready => self
                .email
                .as_ref()
                .expect("ready email view")
                .view_themed(self.theme_overrides.get(&mail.id).copied()),
            Body::Failed(error) => widget::column::with_capacity(2)
                .padding(12)
                .spacing(8)
                .push(widget::text(fl!("preview-failed", error = error.as_str())))
                .push(widget::button::text(fl!("mail-retry")).on_press(Message::Retry))
                .into(),
            Body::Empty | Body::Loading => {
                widget::container(widget::text::body(fl!("mail-body-loading")))
                    .padding(12)
                    .into()
            }
        };
        widget::container(
            widget::column::with_capacity(2)
                .push(header)
                .push(body)
                .spacing(crate::features::mailbox::CONTENT_GUTTER)
                .height(Length::Fill),
        )
        .class(cosmic::theme::Container::Card)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn header<'a>(
        &'a self,
        mail: &'a Mail,
        folder: Folder,
        config: &Config,
        photo: Option<&cosmic::iced::widget::image::Handle>,
        width: f32,
    ) -> Element<'a, Message> {
        let style = config.message_actions;
        let recipients = mail.recipients.as_deref().unwrap_or(&self.recipients);
        let mut sender = widget::column::with_capacity(2)
            .push(widget::text::heading(mail.sender.as_str()))
            .width(Length::Fill)
            .spacing(2);
        if !mail.address.is_empty() && mail.address != mail.sender {
            sender = sender.push(address_menu(&mail.address, &mail.address, false));
        } else if !mail.address.is_empty() {
            sender = widget::column::with_capacity(1)
                .width(Length::Fill)
                .push(address_menu(&mail.address, &mail.address, false));
        }

        let sender: Element<_> = if config.show_recipient_pictures {
            widget::row::with_capacity(2)
                .width(Length::Fill)
                .spacing(8)
                .align_y(cosmic::iced::Alignment::Center)
                .push(mail.sender_picture(36.0, photo))
                .push(sender)
                .into()
        } else {
            sender.into()
        };

        let move_to = |destination| {
            (folder != destination).then_some(Message::Change(remote::Change::Move(destination)))
        };
        let actions = widget::row::with_children([
            action_button(
                style,
                "mail-reply-all-symbolic",
                fl!("reply"),
                (!self.reply_address(mail).is_empty()).then_some(Message::Reply),
            ),
            action_button(
                style,
                "mail-forward-symbolic",
                fl!("forward"),
                matches!(self.body, Body::Ready).then_some(Message::Forward),
            ),
            action_button(
                style,
                "mail-archive-symbolic",
                fl!("archive-message"),
                move_to(Folder::Archive),
            ),
            action_button(
                style,
                "mail-mark-junk-symbolic",
                fl!("spam-message"),
                move_to(Folder::Junk),
            ),
            action_button(
                style,
                "user-trash-symbolic",
                fl!("delete-message"),
                move_to(Folder::Trash),
            ),
            self.more_menu(mail, style),
            action_button(
                style,
                if mail.flagged {
                    "mail-mark-important-symbolic"
                } else {
                    "insert-star-symbolic"
                },
                if mail.flagged {
                    fl!("unstar-message")
                } else {
                    fl!("star-message")
                },
                Some(Message::Change(remote::Change::Flagged(!mail.flagged))),
            ),
        ])
        .spacing(4)
        .wrap();

        let mut recipient_row = widget::row::with_capacity(recipients.len() + 1)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(4);
        if !recipients.is_empty() {
            recipient_row = recipient_row.push(widget::text::body(fl!("recipient")));
            for recipient in recipients {
                let label = match (recipient.name.as_str(), recipient.address.as_str()) {
                    ("", address) => address.to_owned(),
                    (name, "") => name.to_owned(),
                    (name, address) if name == address => address.to_owned(),
                    (name, address) => format!("{name} <{address}>"),
                };
                recipient_row = recipient_row.push(if recipient.address.is_empty() {
                    widget::text::body(label).into()
                } else {
                    address_menu(label, &recipient.address, true)
                });
            }
        }
        let metadata = widget::row::with_capacity(2)
            .spacing(8)
            .push(widget::container(recipient_row.wrap()).width(Length::Fill))
            .push(widget::text::caption(mail.display_time()));

        let mut header = widget::column::with_capacity(4).spacing(8);
        let inline_width = match style {
            MessageActions::IconOnly => INLINE_ACTIONS_MIN_WIDTH,
            MessageActions::TextOnly => 675.0,
            MessageActions::IconAndText => 825.0,
        };
        let inline_actions = width >= inline_width;
        let trailing_actions = if inline_actions {
            header = header.push(
                widget::row::with_capacity(2)
                    .spacing(12)
                    .push(sender)
                    .push(actions),
            );
            None
        } else {
            header = header.push(sender);
            Some(actions)
        };
        header = header
            .push(metadata)
            .push(widget::text::title2(mail.subject.as_str()));
        if let Some(actions) = trailing_actions {
            header = header.push(actions);
        }
        if !self.attachments.is_empty() {
            let mut files = widget::row::with_capacity(self.attachments.len()).spacing(6);
            for (index, file) in self.attachments.iter().enumerate() {
                let size = fl!(
                    "attachment-size",
                    size = format!("{:.1}", file.size as f64 / 1024.0)
                );
                files = files.push(
                    widget::button::standard(fl!(
                        "mail-attachment-label",
                        name = file.name.as_str(),
                        size = size.as_str()
                    ))
                    .leading_icon(widget::icon::from_name("mail-attachment-symbolic"))
                    .on_press(Message::SaveAttachment(self.request, index)),
                );
            }
            header = header
                .push(widget::text::caption(fl!("mail-attachments")))
                .push(
                    widget::container(widget::scrollable(files.wrap()).height(Length::Shrink))
                        .max_height(112),
                );
        }
        header.into()
    }
    fn more_menu(&self, mail: &Mail, style: MessageActions) -> Element<'static, Message> {
        let label = if mail.unread {
            fl!("mark-message-read")
        } else {
            fl!("mark-message-unread")
        };
        let mut items = vec![(
            label,
            Some(Message::Change(remote::Change::Seen(mail.unread))),
        )];
        if matches!(self.body, Body::Ready)
            && self
                .email
                .as_ref()
                .is_some_and(|view| !view.remote_images_enabled())
        {
            items.push((
                fl!("mail-show-remote-images"),
                Some(Message::ShowRemoteImages),
            ));
        }
        if matches!(self.body, Body::Ready) {
            let theme_override = self.theme_overrides.get(&mail.id).copied();
            let is_dark = theme_override.unwrap_or_else(cosmic::theme::is_dark);
            items.push((
                if is_dark {
                    fl!("mail-use-light-theme")
                } else {
                    fl!("mail-use-dark-theme")
                },
                Some(Message::SetTheme(mail.id.clone(), Some(!is_dark))),
            ));
            if theme_override.is_some() {
                items.push((
                    fl!("mail-use-desktop-theme"),
                    Some(Message::SetTheme(mail.id.clone(), None)),
                ));
            }
        }
        let root = action_button(
            style,
            "view-more-symbolic",
            fl!("more-message-actions"),
            Some(Message::Wake),
        );
        dropdown_menu(root, items)
    }
}

fn action_button(
    style: MessageActions,
    icon: &'static str,
    label: String,
    message: Option<Message>,
) -> Element<'static, Message> {
    match style {
        MessageActions::IconOnly => widget::button::icon(widget::icon::from_name(icon))
            .tooltip(label)
            .on_press_maybe(message)
            .into(),
        MessageActions::TextOnly => widget::button::standard(label)
            .on_press_maybe(message)
            .into(),
        MessageActions::IconAndText => widget::button::standard(label)
            .leading_icon(widget::icon::from_name(icon))
            .on_press_maybe(message)
            .into(),
    }
}

fn address_menu(
    label: impl Into<String>,
    address: &str,
    recipient: bool,
) -> Element<'static, Message> {
    let label =
        cosmic::iced::widget::rich_text::<(), _, _, _>([
            cosmic::iced::widget::span(label.into()).underline(true)
        ])
        .class(cosmic::theme::Text::Accent);
    let label = if recipient {
        label
            .size(14.0)
            .line_height(cosmic::iced::widget::text::LineHeight::Absolute(
                21.0.into(),
            ))
    } else {
        label
    };
    let root = Element::from(
        widget::button::custom(label)
            .class(cosmic::theme::Button::MenuRoot)
            .padding(if recipient { [0, 0] } else { [2, 4] })
            .force_enabled(true),
    );
    dropdown_menu(
        root,
        vec![
            (
                fl!("copy-email-address"),
                Some(Message::CopyAddress(address.to_owned())),
            ),
            (
                fl!("compose-to-address"),
                Some(Message::ComposeTo(address.to_owned())),
            ),
        ],
    )
}

fn dropdown_menu(
    root: Element<'static, Message>,
    items: Vec<(String, Option<Message>)>,
) -> Element<'static, Message> {
    let children = items
        .into_iter()
        .map(|(label, message)| {
            menu::Tree::new(Element::from(
                menu::menu_button(vec![widget::text(label).into()]).on_press_maybe(message),
            ))
        })
        .collect();
    menu::bar(vec![menu::Tree::with_children(root, children)])
        .item_width(menu::ItemWidth::Uniform(220))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_list_excludes_inline_parts_and_preserves_the_part_index() {
        let raw = concat!(
            "From: sender@example.org\r\nMIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=mail\r\n\r\n",
            "--mail\r\nContent-Type: text/plain\r\n\r\nHello\r\n",
            "--mail\r\nContent-Type: image/png\r\n",
            "Content-Disposition: inline; filename=logo.png\r\n\r\ninline\r\n",
            "--mail\r\nContent-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=\"../report.bin\"\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--mail--\r\n",
        )
        .as_bytes();

        let message = MessageParser::new().parse(raw).expect("MIME message");
        let attachments = Attachment::list(&message);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].name, "report.bin");
        assert_eq!(attachments[0].size, 3);
        assert_eq!(
            message
                .attachments()
                .nth(attachments[0].part)
                .unwrap()
                .decoded()
                .as_ref(),
            &[0, 1, 2]
        );
    }
}
