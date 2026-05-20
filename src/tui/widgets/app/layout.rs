use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Margin;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Widget;

use crate::tui::app::AppFocus;

pub(super) const TABLIST_WIDTH: u16 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AppPaneLayout {
    pub body: Rect,
    pub tablist: AppPane,
    pub info: AppPane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AppPane {
    Hidden,
    Inline(Rect),
    Overlay(Rect),
}

impl AppPane {
    pub fn area(self) -> Option<Rect> {
        match self {
            Self::Hidden => None,
            Self::Inline(area) | Self::Overlay(area) => Some(area),
        }
    }

    /// renders block (dims background if overlay), returns inner
    pub(super) fn prerender(
        self,
        body_area: Rect,
        focused: bool,
        buf: &mut Buffer,
    ) -> Option<Rect> {
        let area = self.area()?;
        if matches!(self, Self::Overlay(_)) {
            dim_area(body_area, buf);
            Clear.render(area, buf);
        }
        Some(prerender_block(area, focused, buf))
    }
}

impl AppPaneLayout {
    pub(super) fn has_inline_panes(&self) -> bool {
        matches!(self.tablist, AppPane::Inline(_)) || matches!(self.info, AppPane::Inline(_))
    }

    pub(super) fn new(
        area: Rect,
        message_width: u16,
        tablist_width: u16,
        info_width: u16,
        focus: AppFocus,
    ) -> Self {
        let info_fits = message_width + info_width <= area.width;
        let tablist_fits = message_width + tablist_width <= area.width;
        let both_fit = message_width + tablist_width + info_width <= area.width;

        let tablist_inline = both_fit || (!info_fits && tablist_fits);
        let info_inline = both_fit || info_fits;

        let mut body = area;
        let tablist = if tablist_inline {
            AppPane::Inline(take_left(&mut body, tablist_width))
        } else {
            overlay_left(area, tablist_width, focus == AppFocus::Tabs)
        };
        let info = if info_inline {
            AppPane::Inline(take_right(&mut body, info_width))
        } else {
            overlay_right(area, info_width, focus == AppFocus::Info)
        };

        let body = constrain_centered(area, message_width, body);

        Self {
            body,
            tablist,
            info,
        }
    }

    pub(super) fn prerender_body(
        self,
        focused: bool,
        buf: &mut Buffer,
    ) -> Rect {
        if self.has_inline_panes() {
            prerender_block(self.body, focused, buf)
        } else {
            self.body
        }
    }
}

fn take_left(
    area: &mut Rect,
    width: u16,
) -> Rect {
    let [taken, rest] =
        Layout::horizontal([Constraint::Length(width), Constraint::Fill(1)]).areas(*area);
    *area = rest;
    taken
}

fn take_right(
    area: &mut Rect,
    width: u16,
) -> Rect {
    let [rest, taken] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(width)]).areas(*area);
    *area = rest;
    taken
}

pub(super) fn constrain_centered(
    area: Rect,
    width: u16,
    body: Rect,
) -> Rect {
    if body.width <= width {
        return body;
    }
    let ideal_x = area.x + (area.width - width) / 2;
    let max_x = body.x + body.width - width;
    Rect {
        x: ideal_x.clamp(body.x, max_x),
        width,
        ..area
    }
}

fn overlay_left(
    mut area: Rect,
    width: u16,
    visible: bool,
) -> AppPane {
    if visible {
        AppPane::Overlay(take_left(&mut area, width))
    } else {
        AppPane::Hidden
    }
}

fn overlay_right(
    mut area: Rect,
    width: u16,
    visible: bool,
) -> AppPane {
    if visible {
        AppPane::Overlay(take_right(&mut area, width))
    } else {
        AppPane::Hidden
    }
}

pub(super) fn prerender_block(
    area: Rect,
    focused: bool,
    buf: &mut Buffer,
) -> Rect {
    let mut block = Block::default().borders(Borders::ALL);
    let mut style = Style::default();
    if !focused {
        style = style.add_modifier(Modifier::DIM);
    }
    block = block.border_style(style);

    let inner = block.inner(area).inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    block.render(area, buf);
    inner
}

fn dim_area(
    area: Rect,
    buf: &mut Buffer,
) {
    for pos in area.positions() {
        let cell = &mut buf[pos];
        cell.set_style(cell.style().add_modifier(Modifier::DIM));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use similar_asserts::assert_eq;

    use super::*;

    const MESSAGE_WIDTH: u16 = 120;
    const INFO_WIDTH: u16 = 40;
    const HEIGHT: u16 = 10;

    fn layout(
        width: u16,
        focus: AppFocus,
    ) -> AppPaneLayout {
        AppPaneLayout::new(
            Rect::new(0, 0, width, HEIGHT),
            MESSAGE_WIDTH,
            TABLIST_WIDTH,
            INFO_WIDTH,
            focus,
        )
    }

    #[test]
    fn renders_both_panes_inline_when_they_fit() {
        assert_eq!(
            layout(184, AppFocus::Body),
            AppPaneLayout {
                body: Rect::new(24, 0, 120, 10),
                tablist: AppPane::Inline(Rect::new(0, 0, 24, 10)),
                info: AppPane::Inline(Rect::new(144, 0, 40, 10)),
            }
        );
    }

    #[test]
    fn info_pane_wins_when_only_one_pane_fits() {
        assert_eq!(
            layout(160, AppFocus::Body),
            AppPaneLayout {
                body: Rect::new(0, 0, 120, 10),
                tablist: AppPane::Hidden,
                info: AppPane::Inline(Rect::new(120, 0, 40, 10)),
            }
        );
    }

    #[test]
    fn tablist_wins_when_info_pane_cannot_fit() {
        assert_eq!(
            layout(144, AppFocus::Body),
            AppPaneLayout {
                body: Rect::new(24, 0, 120, 10),
                tablist: AppPane::Inline(Rect::new(0, 0, 24, 10)),
                info: AppPane::Hidden,
            }
        );
    }

    #[test]
    fn focused_tablist_overlays_when_info_pane_wins_inline_space() {
        assert_eq!(
            layout(160, AppFocus::Tabs),
            AppPaneLayout {
                body: Rect::new(0, 0, 120, 10),
                tablist: AppPane::Overlay(Rect::new(0, 0, 24, 10)),
                info: AppPane::Inline(Rect::new(120, 0, 40, 10)),
            }
        );
    }

    #[test]
    fn focused_info_pane_overlays_when_tablist_wins_inline_space() {
        assert_eq!(
            layout(144, AppFocus::Info),
            AppPaneLayout {
                body: Rect::new(24, 0, 120, 10),
                tablist: AppPane::Inline(Rect::new(0, 0, 24, 10)),
                info: AppPane::Overlay(Rect::new(104, 0, 40, 10)),
            }
        );
    }

    #[test]
    fn hides_both_panes_when_neither_fits() {
        assert_eq!(
            layout(119, AppFocus::Body),
            AppPaneLayout {
                body: Rect::new(0, 0, 119, 10),
                tablist: AppPane::Hidden,
                info: AppPane::Hidden,
            }
        );
    }

    #[test]
    fn centers_body_when_terminal_is_wider_than_message() {
        assert_eq!(
            layout(300, AppFocus::Body),
            AppPaneLayout {
                body: Rect::new(90, 0, 120, 10),
                tablist: AppPane::Inline(Rect::new(0, 0, 24, 10)),
                info: AppPane::Inline(Rect::new(260, 0, 40, 10)),
            }
        );
    }

    #[test]
    fn clamps_centered_body_against_info_pane() {
        assert_eq!(
            layout(170, AppFocus::Body),
            AppPaneLayout {
                body: Rect::new(10, 0, 120, 10),
                tablist: AppPane::Hidden,
                info: AppPane::Inline(Rect::new(130, 0, 40, 10)),
            }
        );
    }
}
