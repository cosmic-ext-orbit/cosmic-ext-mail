// SPDX-License-Identifier: GPL-3.0-only

use super::EmailView;
use cosmic::iced::advanced::image::{self, Renderer as _};
use cosmic::iced::advanced::{
    Clipboard, Layout, Shell, clipboard, layout, renderer,
    widget::{Tree, Widget},
};
use cosmic::iced::{Event, Length, Rectangle, Size, keyboard, mouse, window};
use cosmic::{Renderer, Theme};
use servo::{
    ClipboardAction, DevicePoint, InputEvent, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};

pub(super) struct Pane<'a>(pub(super) &'a EmailView);

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
            self.0.runtime.wake();
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
