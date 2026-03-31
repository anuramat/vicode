use super::Tab;

impl Tab<'_> {
    pub fn scroll_line_up(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.line_up(history);
    }

    pub fn scroll_line_down(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.line_down(history);
    }

    pub fn scroll_half_page_up(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.half_page_up(history);
    }

    pub fn scroll_half_page_down(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.half_page_down(history);
    }

    pub fn scroll_page_up(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.page_up(history);
    }

    pub fn scroll_page_down(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.page_down(history);
    }

    pub fn scroll_prev_element(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.prev_element(history);
    }

    pub fn scroll_next_element(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.next_element(history);
    }

    pub fn scroll_top(&mut self) {
        let history = &self.agent.state.context.history;
        self.scroll.top(history);
    }

    pub fn scroll_bottom(&mut self) {
        self.scroll.bottom();
    }
}
