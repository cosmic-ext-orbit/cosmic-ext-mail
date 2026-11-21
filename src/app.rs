// SPDX-License-Identifier: GPL-3.0-only

use crate::fl;
use cosmic::iced::{Length, Subscription};
use cosmic::prelude::*;
use cosmic::widget;
use cosmic_ext_mail::{EmailBody, EmailView};

pub struct AppModel {
    core: cosmic::Core,
    preview: Result<EmailView, String>,
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = ();

    const APP_ID: &'static str = "dev.edfloreshz.Mail";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _: ()) -> (Self, Task<cosmic::Action<()>>) {
        let mut app = Self {
            core,
            preview: EmailView::new(EmailBody::Html(include_str!(
                "../resources/sample-email.html"
            ))),
        };
        let title = app
            .core
            .main_window_id()
            .map_or_else(Task::none, |id| app.set_window_title(fl!("app-title"), id));
        (app, title)
    }

    fn subscription(&self) -> Subscription<()> {
        self.preview
            .as_ref()
            .map_or_else(|_| Subscription::none(), EmailView::subscription)
    }

    fn view(&self) -> Element<'_, ()> {
        let body = match &self.preview {
            Ok(preview) => preview.view(),
            Err(error) => widget::text(fl!("preview-failed", error = error.as_str())).into(),
        };

        widget::column::with_capacity(3)
            .push(widget::text::title2(fl!("sample-subject")))
            .push(widget::text(fl!("sample-sender")))
            .push(body)
            .spacing(12)
            .padding(24)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
