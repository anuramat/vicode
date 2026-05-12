use anyhow::Result;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;

use crate::tui::app::App;

impl App<'_> {
    pub async fn key(
        &mut self,
        event: KeyEvent,
    ) -> Result<()> {
        if event.kind != KeyEventKind::Press {
            // apparently windows sends key release events too; not that I care but just in case:
            return Ok(());
        }
        // TODO add failsafe, so that there's always a way to exit the app even if the keymap is messed up (e.g. spam ctrl-c to quit)
        let keymap = &self.project.config().keymap;
        if self.cmdline.input.focused() {
            if let Some(command) = keymap.cmdline(event) {
                self.execute(command).await?;
            } else {
                self.cmdline.input.handle(event);
            }
        } else if self.selected_tab().is_ok_and(|tab| tab.input.focused()) {
            if let Some(command) = keymap.insert(event) {
                self.execute(command).await?;
            } else {
                self.selected_tab_mut()?.key_insert(event);
            }
        } else if let Some(command) = keymap.normal(event) {
            self.execute(command).await?;
        }
        Ok(())
    }
}
