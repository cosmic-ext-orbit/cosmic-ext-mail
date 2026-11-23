// SPDX-License-Identifier: GPL-3.0-only

mod body;
mod document;
mod pane;
mod runtime;

pub use body::{EmailBody, PreparedEmail};
use cosmic::iced::advanced::image::{self, Renderer as _};
use cosmic::iced::{Rectangle, Subscription};
use cosmic::{Element, Renderer};
use document::Delegate;
use pane::Pane;
use runtime::Runtime;
use servo::{
    DeviceIntPoint, DeviceIntRect, DeviceIntSize, RenderingContext, SoftwareRenderingContext,
    UserContentManager, WebView, WebViewBuilder, user_contents::UserStyleSheet,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// Center outer layout boxes while leaving sender-defined styles and nested tables in control.
const READER_STYLES: &str = "
    body > :is(div, main, section):not([align]),
    table:not(table table, [align]) {
        margin-inline: auto;
    }
";

const DARK_READER_STYLES: &str = "
    html {
        color-scheme: dark;
        background-color: #18181b;
        color: #f7f7f8;
    }
    a:link { color: #8ab4f8; }
    a:visited { color: #c6a0f6; }
";

// This fixed embedder script runs independently of the message's blocked scripts.
const ADAPT_DARK_MESSAGE: &str = include_str!("dark_reader.js");

#[derive(Clone, Copy, PartialEq, Eq)]
enum DarkAdaptation {
    Pending,
    Running,
    Complete,
}

/// A live, read-only COSMIC pane for HTML, plain text, or raw MIME email.
pub struct EmailView {
    webview: WebView,
    runtime: Rc<Runtime>,
    context: Rc<SoftwareRenderingContext>,
    delegate: Rc<Delegate>,
    content_manager: Rc<UserContentManager>,
    dark_styles: RefCell<Option<Rc<UserStyleSheet>>>,
    dark_adaptation: Rc<Cell<DarkAdaptation>>,
    frame: RefCell<Option<image::Allocation>>,
    scale: Cell<f32>,
    focused: Cell<bool>,
    dragging: Cell<bool>,
}

impl EmailView {
    /// Creates a pane on the COSMIC application's UI thread. Panes share one Servo runtime.
    pub fn new(body: EmailBody<'_>) -> Result<Self, String> {
        Self::new_with_remote_images(body, false)
    }

    pub fn new_with_remote_images(
        body: EmailBody<'_>,
        remote_images: bool,
    ) -> Result<Self, String> {
        Self::new_with_appearance(body, remote_images, false)
    }

    pub fn new_with_appearance(
        body: EmailBody<'_>,
        remote_images: bool,
        dark_mode: bool,
    ) -> Result<Self, String> {
        Self::from_prepared(body.prepare()?, remote_images, dark_mode)
    }

    /// Creates a preview from content prepared off the UI thread.
    pub fn from_prepared(
        body: PreparedEmail,
        remote_images: bool,
        dark_mode: bool,
    ) -> Result<Self, String> {
        let PreparedEmail(html) = body;
        let context = Rc::new(
            SoftwareRenderingContext::new((640_u32, 420_u32).into()).map_err(|error| {
                format!("Could not create Servo's rendering context: {error:?}")
            })?,
        );
        context
            .make_current()
            .map_err(|error| format!("{error:?}"))?;
        let runtime = Runtime::shared();
        let url = runtime.message_url();
        let delegate = Rc::new(Delegate::new(url.clone(), html, remote_images));
        let content_manager = Rc::new(UserContentManager::new(&runtime.servo));
        content_manager.add_stylesheet(Rc::new(UserStyleSheet::new(
            READER_STYLES.into(),
            url.clone(),
        )));
        let dark_styles = dark_mode.then(|| {
            let stylesheet = Rc::new(UserStyleSheet::new(DARK_READER_STYLES.into(), url.clone()));
            content_manager.add_stylesheet(stylesheet.clone());
            stylesheet
        });
        let webview = WebViewBuilder::new(&runtime.servo, context.clone())
            .user_content_manager(content_manager.clone())
            .url(url)
            .delegate(delegate.clone())
            .clipboard_delegate(delegate.clone())
            .build();
        webview.notify_theme_change(if dark_mode {
            servo::Theme::Dark
        } else {
            servo::Theme::Light
        });
        webview.show();

        Ok(Self {
            webview,
            runtime,
            context,
            delegate,
            content_manager,
            dark_styles: RefCell::new(dark_styles),
            dark_adaptation: Rc::new(Cell::new(DarkAdaptation::Pending)),
            frame: RefCell::new(None),
            scale: Cell::new(1.0),
            focused: Cell::new(false),
            dragging: Cell::new(false),
        })
    }

    /// Replaces the content without restarting Servo. Parse failures leave the current message intact.
    pub fn set_body(&mut self, body: EmailBody<'_>) -> Result<(), String> {
        self.set_body_with_remote_images(body, false)
    }

    pub fn set_body_with_remote_images(
        &mut self,
        body: EmailBody<'_>,
        remote_images: bool,
    ) -> Result<(), String> {
        self.set_prepared(body.prepare()?, remote_images);
        Ok(())
    }

    /// Displays previously prepared content in this preview.
    pub fn set_prepared(&mut self, body: PreparedEmail, remote_images: bool) {
        let PreparedEmail(html) = body;
        let url = self.runtime.message_url();
        self.delegate
            .replace_document(url.clone(), html, remote_images);
        self.load(url);
    }

    /// Toggles the reader's dark appearance without changing the message or image policy.
    fn set_dark_mode(&self, enabled: bool) {
        let mut dark_styles = self.dark_styles.borrow_mut();
        if enabled == dark_styles.is_some() {
            return;
        }
        let url = self.runtime.message_url();
        if enabled {
            let stylesheet = Rc::new(UserStyleSheet::new(DARK_READER_STYLES.into(), url.clone()));
            self.content_manager.add_stylesheet(stylesheet.clone());
            *dark_styles = Some(stylesheet);
        } else if let Some(stylesheet) = dark_styles.take() {
            self.content_manager.remove_stylesheet(stylesheet);
        }
        self.webview.notify_theme_change(if enabled {
            servo::Theme::Dark
        } else {
            servo::Theme::Light
        });
        self.delegate
            .reload_at(url.clone(), self.remote_images_enabled());
        self.load(url);
    }

    /// Allows external images for the current message and reloads it.
    pub fn show_remote_images(&mut self) {
        self.set_remote_images(true);
    }

    /// Updates the external image policy and reloads the current message if it changed.
    pub fn set_remote_images(&mut self, enabled: bool) {
        if enabled != self.remote_images_enabled() {
            let url = self.runtime.message_url();
            self.delegate.reload_at(url.clone(), enabled);
            self.load(url);
        }
    }

    pub fn remote_images_enabled(&self) -> bool {
        self.delegate.remote_images_enabled()
    }

    fn load(&self, url: url::Url) {
        self.dark_adaptation.set(DarkAdaptation::Pending);
        self.frame.borrow_mut().take();
        self.dragging.set(false);
        self.webview.load(url);
        self.runtime.wake();
    }

    pub fn view<Message>(&self) -> Element<'_, Message> {
        Element::new(Pane {
            preview: self,
            appearance: pane::Appearance::Fixed,
        })
    }

    /// Renders message bodies using the COSMIC theme of the enclosing window.
    pub fn view_themed<Message>(&self, dark_override: Option<bool>) -> Element<'_, Message> {
        Element::new(Pane {
            preview: self,
            appearance: pane::Appearance::Themed(dark_override),
        })
    }

    /// Map this wake subscription into the host application's message type.
    pub fn subscription(&self) -> Subscription<()> {
        self.runtime.subscription()
    }

    fn prepare_dark_frame(&self) -> bool {
        if self.dark_styles.borrow().is_none()
            || self.dark_adaptation.get() == DarkAdaptation::Complete
        {
            return true;
        }
        if self.dark_adaptation.get() == DarkAdaptation::Pending
            && self.webview.load_status() == servo::LoadStatus::Complete
            && let Some(url) = self.webview.url()
        {
            self.dark_adaptation.set(DarkAdaptation::Running);
            let state = Rc::clone(&self.dark_adaptation);
            let delegate = Rc::clone(&self.delegate);
            let runtime = Rc::downgrade(&self.runtime);
            self.webview
                .evaluate_javascript(ADAPT_DARK_MESSAGE, move |_| {
                    if delegate.is_current_url(&url) {
                        state.set(DarkAdaptation::Complete);
                        delegate.frame_ready.set(true);
                        if let Some(runtime) = runtime.upgrade() {
                            runtime.wake();
                        }
                    }
                });
        }
        false
    }

    fn update_frame(&self, bounds: Rectangle, renderer: &Renderer) {
        let scale = self.scale.get();
        self.webview
            .set_hidpi_scale_factor(euclid::Scale::new(scale));
        let width = (bounds.width * scale).round().max(1.0) as u32;
        let height = (bounds.height * scale).round().max(1.0) as u32;
        let size = (width, height).into();
        if size != self.context.size() {
            self.delegate.frame_ready.set(false);
            self.webview.resize(size);
        }

        self.runtime.servo.spin_event_loop();
        if self.delegate.frame_ready.replace(false) {
            if !self.delegate.document_is_ready(&self.webview) || !self.prepare_dark_frame() {
                return;
            }
            self.webview.paint();
            let rect = DeviceIntRect::from_origin_and_size(
                DeviceIntPoint::zero(),
                DeviceIntSize::new(width as i32, height as i32),
            );
            if let Some(frame) = self.context.read_to_image(rect) {
                let handle =
                    image::Handle::from_rgba(frame.width(), frame.height(), frame.into_raw());
                // Iced may skip drawing an image until its upload finishes.
                if let Ok(allocation) = renderer.load_image(&handle) {
                    *self.frame.borrow_mut() = Some(allocation);
                }
            }
        }
    }
}
