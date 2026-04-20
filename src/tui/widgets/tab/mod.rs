pub mod input;

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::config::LayoutConfig;
use crate::tui::tab::Focus;
use crate::tui::tab::Tab;
use crate::tui::tab::TabEntry;
use crate::tui::widgets::container::element::RenderContext;

const INPUT_AREA_HEIGHT: u16 = 5;

impl TabEntry<'_> {
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
        layout: LayoutConfig,
    ) {
        match self {
            Self::Loading => render_loading(area, buf),
            Self::Ready(tab) => tab.render(area, buf, ctx, layout),
        }
    }
}

impl Tab<'_> {
    #[tracing::instrument(skip(self, buf))]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ctx: RenderContext,
        layout: LayoutConfig,
    ) {
        let info_width = layout.info_pane_width;
        let body_width = layout.message_width;
        let fits = body_width.saturating_add(info_width) <= area.width;
        let info_focused = self.focus == Focus::Info;

        let (body, info_area) = if fits {
            let [_, body, _, info_area] = *Layout::default()
                .direction(Direction::Horizontal)
                .constraints(vec![
                    Constraint::Fill(1),
                    Constraint::Length(body_width),
                    Constraint::Fill(1),
                    Constraint::Length(info_width),
                ])
                .split(area)
            else {
                unreachable!()
            };
            (body, Some(info_area))
        } else if info_focused {
            let info_area = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(vec![Constraint::Fill(1), Constraint::Length(info_width)])
                .split(area)[1];
            (area, Some(info_area))
        } else {
            (area, None)
        };

        let input_height = if self.input.visible() {
            INPUT_AREA_HEIGHT + 1
        } else {
            0
        };

        let body_inner = if fits {
            let mut block =
                ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL);
            if info_focused {
                block = block.border_style(Style::default().add_modifier(Modifier::DIM));
            }

            let inner = block.inner(body);
            block.render(body, buf);
            inner.inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 0,
            })
        } else {
            body
        };

        let [messages_area, input_area] = *Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Min(0), Constraint::Length(input_height)])
            .split(body_inner)
        else {
            unreachable!()
        };

        self.scroll.render(
            self.agent.state.context.history.state().messages.as_slice(),
            messages_area,
            buf,
            ctx,
        );
        self.input.render(input_area, buf);

        if let Some(info_area) = info_area {
            let mut block =
                ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL);

            if !info_focused {
                block = block.border_style(Style::default().add_modifier(Modifier::DIM));
            } else if !fits {
                // dim the entire body
                for pos in body.positions() {
                    let cell = &mut buf[pos];
                    cell.set_style(cell.style().add_modifier(Modifier::DIM));
                }
                // and render info pane on top
                Clear.render(info_area, buf);
            }

            let inner = block.inner(info_area).inner(ratatui::layout::Margin {
                horizontal: 1,
                vertical: 0,
            });
            block.render(info_area, buf);
            self.info.render(inner, buf);
        }
    }
}

fn render_loading(
    area: Rect,
    buf: &mut Buffer,
) {
    let style = Style::default().add_modifier(Modifier::REVERSED);
    buf.set_style(area, style);

    let text = "LOADING".to_string();
    let area = Rect {
        x: area.x + ((area.width - text.len() as u16) / 2),
        y: area.y + area.height / 2,
        width: text.len() as u16,
        height: 1,
    };
    let widget = Paragraph::new(text);

    widget.render(area, buf);
}
