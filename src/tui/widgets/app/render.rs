use anyhow::Result;
use ratatui::prelude::*;
use ratatui::widgets::Clear;

use super::layout::AppPaneLayout;
use super::layout::TABLIST_WIDTH;
use crate::tui::app::App;
use crate::tui::app::AppFocus;
use crate::tui::widgets::logo::LOGO_VARIANTS;

const CONSTRAINTS: [Constraint; 2] = [Constraint::Min(0), Constraint::Length(1)];

impl App<'_> {
    #[tracing::instrument(skip(self, term))]
    pub fn draw<B>(
        &mut self,
        term: &mut Terminal<B>,
    ) -> Result<()>
    where
        B: ratatui::backend::Backend,
    {
        tracing::debug!("start app render");
        term.draw(|frame| {
            let [body_area, line_area] = *Layout::default()
                .direction(Direction::Vertical)
                .constraints(CONSTRAINTS)
                .split(frame.area())
            else {
                unreachable!();
            };

            let cfg = self.project.config().layout;
            let panes = AppPaneLayout::new(
                body_area,
                cfg.message_width,
                TABLIST_WIDTH,
                cfg.info_pane_width,
                self.focus,
            );

            let ctx = self.ctx;
            if let Some(idx) = self.selected_tab_idx() {
                if let Some((_, tab)) = self.tabs.get_index_mut(idx) {
                    let body =
                        panes.prerender_body(self.focus == AppFocus::Body, frame.buffer_mut());
                    tab.render(body, frame.buffer_mut(), ctx);
                    if let Some(inner) = panes.info.prerender(
                        body_area,
                        self.focus == AppFocus::Info,
                        frame.buffer_mut(),
                    ) {
                        tab.info.render(inner, frame.buffer_mut());
                    }
                }
            } else {
                frame.render_widget(&*LOGO_VARIANTS, body_area);
            }

            if let Some(tablist_area) =
                panes
                    .tablist
                    .prerender(body_area, self.focus == AppFocus::Tabs, frame.buffer_mut())
            {
                frame.render_stateful_widget(
                    &self.tablist.widget,
                    tablist_area,
                    &mut self.tablist.state,
                );
            }

            if self.cmdline.input.focused() {
                self.cmdline.render(line_area, frame.buffer_mut());
            } else {
                frame.render_widget(&Clear, line_area);
                let stl = self.status_line(line_area.width);
                frame.render_widget(&stl, line_area);
            }
        })?;
        tracing::debug!("end app render");
        Ok(())
    }
}
