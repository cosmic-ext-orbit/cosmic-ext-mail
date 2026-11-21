// SPDX-License-Identifier: GPL-3.0-only

use crate::EmailBody;
use cosmic::iced::advanced::image::{self, Renderer as _};
use cosmic::iced::advanced::{
    Clipboard, Layout, Shell, clipboard, layout, renderer,
    widget::{Tree, Widget},
};
use cosmic::iced::{
    Event, Length, Rectangle, Size, Subscription, futures, keyboard, mouse, window,
};
use cosmic::{Element, Renderer, Theme};
use http::{HeaderMap, HeaderValue, header};
use servo::{
    ClipboardAction, ClipboardDelegate, DeviceIntPoint, DeviceIntRect, DeviceIntSize, DevicePoint,
    EventLoopWaker, InputEvent, LoadStatus, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    NavigationRequest, RenderingContext, Servo, ServoBuilder, ServoDelegate,
    SoftwareRenderingContext, WebResourceLoad, WebResourceResponse, WebView, WebViewBuilder,
    WebViewDelegate, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::Notify;
use url::Url;

#[derive(Clone, Default)]
struct WakeSignal(Arc<Notify>);

impl Hash for WakeSignal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl EventLoopWaker for WakeSignal {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        self.0.notify_one();
    }
}

struct Document {
    url: Url,
    html: String,
}

struct Delegate {
    document: RefCell<Document>,
    frame_ready: Cell<bool>,
    copied: RefCell<Option<String>>,
    cursor: Cell<servo::Cursor>,
}

struct BlockBackgroundNetwork;

impl ServoDelegate for BlockBackgroundNetwork {
    fn load_web_resource(&self, load: WebResourceLoad) {
        let url = load.request().url.as_url();
        if matches!(url.scheme(), "http" | "https" | "file") {
            let response = WebResourceResponse::new(url.clone());
            load.intercept(response).cancel();
        }
    }
}

impl WebViewDelegate for Delegate {
    fn request_navigation(&self, _: WebView, request: NavigationRequest) {
        if request.url == self.document.borrow().url {
            request.allow();
        } else {
            request.deny();
        }
    }

    fn load_web_resource(&self, _: WebView, load: WebResourceLoad) {
        let document = self.document.borrow();
        let request = load.request();
        let url = request.url.as_url();
        if !request.is_for_main_frame && url.scheme() == "data" {
            return;
        }
        let mut response = WebResourceResponse::new(url.clone());

        if !request.is_for_main_frame || *url != document.url {
            load.intercept(response).cancel();
            return;
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
            "default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; sandbox",
        ));
        headers.insert("x-dns-prefetch-control", HeaderValue::from_static("off"));
        response.headers = headers;
        load.intercept(response)
            .send_body_data(document.html.as_bytes().to_vec());
    }

    fn notify_new_frame_ready(&self, _: WebView) {
        self.frame_ready.set(true);
    }

    fn notify_load_status_changed(&self, _: WebView, status: LoadStatus) {
        if matches!(status, LoadStatus::Complete) {
            self.frame_ready.set(true);
        }
    }

    fn notify_cursor_changed(&self, _: WebView, cursor: servo::Cursor) {
        self.cursor.set(cursor);
    }
}

impl ClipboardDelegate for Delegate {
    fn set_text(&self, _: WebView, text: String) {
        *self.copied.borrow_mut() = Some(text);
    }
}

struct Runtime {
    servo: Servo,
    wake: WakeSignal,
    next_message_id: Cell<u64>,
}

impl Runtime {
    fn new() -> Self {
        let wake = WakeSignal::default();
        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(wake.clone()))
            .build();
        servo.set_delegate(Rc::new(BlockBackgroundNetwork));
        Self {
            servo,
            wake,
            next_message_id: Cell::new(0),
        }
    }

    fn message_url(&self) -> Url {
        let id = self.next_message_id.get();
        self.next_message_id.set(id + 1);
        Url::parse(&format!("https://cosmic-mail.invalid/message/{id}")).expect("valid message URL")
    }
}

thread_local! {
    static RUNTIME: Rc<Runtime> = Rc::new(Runtime::new());
}

/// A live, read-only COSMIC pane for HTML, plain text, or raw MIME email.
pub struct EmailView {
    webview: WebView,
    runtime: Rc<Runtime>,
    context: Rc<SoftwareRenderingContext>,
    delegate: Rc<Delegate>,
    frame: RefCell<Option<image::Allocation>>,
    scale: Cell<f32>,
    focused: Cell<bool>,
    dragging: Cell<bool>,
}

impl EmailView {
    /// Creates a pane on the COSMIC application's UI thread. Panes share one Servo runtime.
    pub fn new(body: EmailBody<'_>) -> Result<Self, String> {
        let html = body.into_html()?;
        let context = Rc::new(
            SoftwareRenderingContext::new((640_u32, 420_u32).into()).map_err(|error| {
                format!("Could not create Servo's rendering context: {error:?}")
            })?,
        );
        context
            .make_current()
            .map_err(|error| format!("{error:?}"))?;
        let runtime = RUNTIME.with(Rc::clone);
        let url = runtime.message_url();
        let delegate = Rc::new(Delegate {
            document: RefCell::new(Document {
                url: url.clone(),
                html,
            }),
            frame_ready: Cell::new(false),
            copied: RefCell::new(None),
            cursor: Cell::new(servo::Cursor::default()),
        });
        let webview = WebViewBuilder::new(&runtime.servo, context.clone())
            .url(url)
            .delegate(delegate.clone())
            .clipboard_delegate(delegate.clone())
            .build();
        webview.show();

        Ok(Self {
            webview,
            runtime,
            context,
            delegate,
            frame: RefCell::new(None),
            scale: Cell::new(1.0),
            focused: Cell::new(false),
            dragging: Cell::new(false),
        })
    }

    /// Replaces the content without restarting Servo. Parse failures leave the current message intact.
    pub fn set_body(&mut self, body: EmailBody<'_>) -> Result<(), String> {
        let html = body.into_html()?;
        let url = self.runtime.message_url();
        *self.delegate.document.borrow_mut() = Document {
            url: url.clone(),
            html,
        };
        self.frame.get_mut().take();
        self.delegate.frame_ready.set(false);
        self.delegate.copied.borrow_mut().take();
        self.dragging.set(false);
        self.webview.load(url);
        self.runtime.wake.wake();
        Ok(())
    }

    pub fn view<Message>(&self) -> Element<'_, Message> {
        Element::new(Pane(self))
    }

    /// Map this wake subscription into the host application's message type.
    pub fn subscription(&self) -> Subscription<()> {
        Subscription::run_with(self.runtime.wake.clone(), |wake| {
            futures::stream::unfold(wake.clone(), |wake| async move {
                wake.0.notified().await;
                Some(((), wake))
            })
        })
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
            if self.webview.url().as_ref() != Some(&self.delegate.document.borrow().url)
                || !matches!(self.webview.load_status(), LoadStatus::Complete)
            {
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

struct Pane<'a>(&'a EmailView);

impl<Message> Widget<Message, Theme, Renderer> for Pane<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, _: &mut Tree, _: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.resolve(Length::Fill, Length::Fill, Size::ZERO))
    }

    fn draw(
        &self,
        _: &Tree,
        renderer: &mut Renderer,
        _: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let scale = style.scale_factor as f32;
        if self.0.scale.replace(scale) != scale {
            self.0.runtime.wake.wake();
        }
        let bounds = layout.bounds();
        if let Some(frame) = self.0.frame.borrow().as_ref()
            && let Some(clip) = bounds.intersection(viewport)
        {
            renderer.draw_image(image::Image::new(frame.handle().clone()), bounds, clip);
        }
    }

    fn update(
        &mut self,
        _: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _: &Rectangle,
    ) {
        let preview = self.0;
        let bounds = layout.bounds();
        let inside = cursor.is_over(bounds);
        let scale = preview.scale.get();
        let point = cursor.position_from(bounds.position()).map(|position| {
            WebViewPoint::from(DevicePoint::new(position.x * scale, position.y * scale))
        });
        let input = match event {
            Event::Window(window::Event::RedrawRequested(_)) => {
                preview.update_frame(bounds, renderer);
                if let Some(text) = preview.delegate.copied.borrow_mut().take() {
                    clipboard.write(clipboard::Kind::Standard, text);
                }
                return;
            }
            Event::Window(window::Event::Unfocused) => {
                preview.focused.set(false);
                preview.webview.set_focused(false);
                None
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if inside || preview.dragging.get() => {
                point.map(|point| InputEvent::MouseMove(MouseMoveEvent::new(point)))
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                preview.focused.set(inside);
                preview.webview.set_focused(inside);
                if inside {
                    preview.dragging.set(true);
                    point.map(|point| mouse_button(MouseButtonAction::Down, point))
                } else {
                    None
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if preview.dragging.replace(false) =>
            {
                Some(mouse_button(
                    MouseButtonAction::Up,
                    point.unwrap_or_else(|| DevicePoint::zero().into()),
                ))
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside => {
                let (x, y, mode) = match *delta {
                    mouse::ScrollDelta::Lines { x, y } => {
                        (x * 76.0, y * 76.0, WheelMode::DeltaLine)
                    }
                    mouse::ScrollDelta::Pixels { x, y } => (x, y, WheelMode::DeltaPixel),
                };
                point.map(|point| {
                    InputEvent::Wheel(WheelEvent::new(
                        WheelDelta {
                            x: x.into(),
                            y: y.into(),
                            z: 0.0,
                            mode,
                        },
                        point,
                    ))
                })
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if preview.focused.get()
                    && modifiers.control()
                    && key.as_ref() == keyboard::Key::Character("c") =>
            {
                Some(InputEvent::EditingAction(ClipboardAction::Copy))
            }
            _ => None,
        };

        if let Some(input) = input {
            preview.webview.notify_input_event(input);
            shell.capture_event();
            shell.request_redraw();
        }
    }

    fn mouse_interaction(
        &self,
        _: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _: &Rectangle,
        _: &Renderer,
    ) -> mouse::Interaction {
        if !cursor.is_over(layout.bounds()) {
            return mouse::Interaction::None;
        }
        match self.0.delegate.cursor.get() {
            servo::Cursor::Text | servo::Cursor::VerticalText => mouse::Interaction::Text,
            servo::Cursor::Pointer => mouse::Interaction::Pointer,
            _ => mouse::Interaction::Idle,
        }
    }
}

fn mouse_button(action: MouseButtonAction, point: WebViewPoint) -> InputEvent {
    InputEvent::MouseButton(MouseButtonEvent::new(
        action,
        servo::MouseButton::Primary,
        point,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires a desktop rendering context and local sockets"]
    fn switches_messages_without_loading_remote_resources() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let remote = format!("http://{}/tracking", server.local_addr().unwrap());
        let html = format!(
            r#"<!doctype html><body style="background:#123456"><img src="{remote}">
            <style>body {{ background-image: url("{remote}"); }}</style></body>"#,
        );
        let mut pane = EmailView::new(EmailBody::Html(&html)).unwrap();
        wait_for_color(&pane, [0x12, 0x34, 0x56]);

        pane.set_body(EmailBody::Html(
            r#"<!doctype html><body style="background:#654321">Second message</body>"#,
        ))
        .unwrap();
        wait_for_color(&pane, [0x65, 0x43, 0x21]);
        assert_eq!(
            server.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    fn wait_for_color(pane: &EmailView, expected: [u8; 3]) {
        let rectangle = DeviceIntRect::from_origin_and_size(
            DeviceIntPoint::new(320, 210),
            DeviceIntSize::new(1, 1),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            pane.runtime.servo.spin_event_loop();
            if pane.delegate.frame_ready.replace(false) {
                pane.webview.paint();
                if let Some(frame) = pane.context.read_to_image(rectangle)
                    && frame.get_pixel(0, 0).0[..3] == expected
                {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("Servo did not paint the expected message color {expected:?}");
    }
}
