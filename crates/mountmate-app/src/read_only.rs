use iced::advanced::{Layout, Widget, layout, mouse, renderer, widget::Tree};
use iced::{Element, Length, Rectangle, Renderer, Size, Theme};

/// Preserve the form's layout and drawing while withholding events, focus
/// operations and interactive overlays. Wrap this inside the scroll view so
/// long forms can still be inspected while mounted.
pub fn freeze<'a, Message: 'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    Element::new(ReadOnly { content })
}

struct ReadOnly<'a, Message> {
    content: Element<'a, Message>,
}

impl<Message> Widget<Message, Theme, Renderer> for ReadOnly<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            mouse::Cursor::Unavailable,
            viewport,
        );
    }
}
