use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

// TODO OnceCell doesn't work because type is invariant, maybe find smth else?
use crate::tui::widgets::container::element::*;
use crate::tui::widgets::syntax::HIGHLIGHTER;

self_cell::self_cell!(
    pub struct MarkdownWidgetCell {
        owner: String,
        #[covariant]
        dependent: MarkdownWidgetCellDependent,
    }
    impl {Debug}
);

#[derive(Debug, Clone)]
pub struct MarkdownWidgetCellDependent<'a> {
    pub highlighted: Option<Paragraph<'a>>,
    pub rendered: Option<Paragraph<'a>>,
}

#[derive(Debug)]
pub struct MarkdownWidget(pub MarkdownWidgetCell);

impl HeightComputable for MarkdownWidget {
    fn height(
        &mut self,
        width: u16,
        ctx: RenderContext,
    ) -> u16 {
        self.0.with_dependent_mut(|s, dependent| {
            if ctx.render_markdown {
                if let Some(rendered) = &dependent.rendered {
                    rendered.line_count(width) as u16
                } else {
                    let rendered =
                        Paragraph::new(tui_markdown::from_str(s)).wrap(Wrap { trim: false });
                    let value = rendered.line_count(width) as u16;
                    dependent.rendered = Some(rendered);
                    value
                }
            } else if let Some(highlighted) = &dependent.highlighted {
                highlighted.line_count(width) as u16
            } else {
                let highlighted = Paragraph::new(HIGHLIGHTER.highlight(s, &HIGHLIGHTER.markdown));
                let value = highlighted.line_count(width) as u16;
                dependent.highlighted = Some(highlighted.wrap(Wrap { trim: false }));
                value
            }
        })
    }

    fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
    ) {
        if ctx.render_markdown {
            self.0.borrow_dependent().rendered.render_ref(area, buf);
        } else {
            self.0.borrow_dependent().highlighted.render_ref(area, buf);
        }
    }
}

impl From<String> for MarkdownWidget {
    fn from(value: String) -> Self {
        let cell = MarkdownWidgetCell::new(value, |_| MarkdownWidgetCellDependent {
            rendered: None,
            highlighted: None,
        });
        MarkdownWidget(cell)
    }
}
