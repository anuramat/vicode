use super::Tab;

impl Tab<'_> {
    pub fn scroll_line_up(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.line_up(messages);
    }

    pub fn scroll_line_down(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.line_down(messages);
    }

    pub fn scroll_half_page_up(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.half_page_up(messages);
    }

    pub fn scroll_half_page_down(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.half_page_down(messages);
    }

    pub fn scroll_page_up(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.page_up(messages);
    }

    pub fn scroll_page_down(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.page_down(messages);
    }

    pub fn scroll_prev_element(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.prev_element(messages);
    }

    pub fn scroll_next_element(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.next_element(messages);
    }

    pub fn scroll_top(&mut self) {
        let messages = self.state.context.history.state().messages.as_slice();
        self.scroll.top(messages);
    }

    pub fn scroll_bottom(&mut self) {
        self.scroll.bottom();
    }
}
