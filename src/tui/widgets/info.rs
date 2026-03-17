use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use tokio::process::Command;

use crate::agent::id::AgentId;
use crate::project::PROJECT;
use crate::tui::widgets::container::composite::CompositeElement;
use crate::tui::widgets::container::element::HeightComputable;

#[derive(Clone, Debug, Default)]
pub struct InfoWidget {
    elements: CompositeElement,
}

impl InfoWidget {
    pub async fn new(aid: &AgentId) -> Result<Self> {
        // TODO move command to config
        let args = vec!["-c", "color.status=always", "status", "--short"];
        let output = Command::new("git")
            .current_dir(PROJECT.agent_workdir(aid))
            .args(args)
            .output()
            .await?;
        let text = output.stdout.into_text()?;
        let elements = vec![Paragraph::new(text).into()];
        let elements = CompositeElement(elements);
        Ok(InfoWidget { elements })
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        self.elements.render(area, buf, Default::default());
    }
}
