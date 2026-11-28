//! Animations at a calm frame rate.
//!
//! A spinner asks for the next frame as soon as it is drawn, which redraws
//! the whole window at the monitor's rate (144 Hz and more) for as long as
//! the agent thinks. Wrapped in [`paced`], it asks for one at most every
//! [`FRAME`], which still looks smooth and costs a fraction of the CPU.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Operation, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::time::{Duration, Instant};
use iced::window::RedrawRequest;
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse};

/// The time between frames of a paced animation (about 30 per second).
pub const FRAME: Duration = Duration::from_millis(33);

/// `content`, whose requests for the next frame wait for [`FRAME`].
pub fn paced<'a, Message: 'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Paced {
        content: content.into(),
    })
}

struct Paced<'a, Message> {
    content: Element<'a, Message>,
}

/// What the content asked for, slowed down: a request for the very next
/// frame becomes one for `now + FRAME`, unless something else asked sooner.
pub fn calm(before: RedrawRequest, after: RedrawRequest, now: Instant) -> RedrawRequest {
    match after {
        RedrawRequest::NextFrame if before != RedrawRequest::NextFrame => {
            before.min(RedrawRequest::At(now + FRAME))
        }
        other => other,
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Paced<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let before = shell.redraw_request();
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
        let after = shell.redraw_request();
        Shell::replace_redraw_request(shell, calm(before, after, Instant::now()));
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_frame_waits_for_a_frame_time() {
        let now = Instant::now();
        let at = |ms| RedrawRequest::At(now + Duration::from_millis(ms));
        // Asked for at once: in 33 ms instead.
        assert_eq!(
            calm(RedrawRequest::Wait, RedrawRequest::NextFrame, now),
            at(33)
        );
        // Something else wanted one sooner: that stays.
        assert_eq!(calm(at(10), RedrawRequest::NextFrame, now), at(10));
        assert_eq!(calm(at(50), RedrawRequest::NextFrame, now), at(33));
        // Another widget had already asked for the next frame.
        assert_eq!(
            calm(RedrawRequest::NextFrame, RedrawRequest::NextFrame, now),
            RedrawRequest::NextFrame
        );
        // Nothing asked, or asked for later: unchanged.
        assert_eq!(
            calm(RedrawRequest::Wait, RedrawRequest::Wait, now),
            RedrawRequest::Wait
        );
        assert_eq!(calm(RedrawRequest::Wait, at(500), now), at(500));
    }
}
