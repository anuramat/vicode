use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use tokio::process::Command;

use crate::agent::id::AgentId;
use crate::deps;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::widgets::container::composite::CompositeElement;
use crate::tui::widgets::container::element::HeightComputable;
use crate::tui::widgets::container::element::RenderContext;

#[derive(Debug, Default)]
pub struct InfoWidget {
    elements: CompositeElement,
}

impl InfoWidget {
    pub async fn new(
        project: &Project,
        aid: &AgentId,
    ) -> Result<Self> {
        let args = vec!["-c".to_string(), project.config().info_cmd.clone()];
        let output = Command::new(deps::BASH)
            .current_dir(project.agent_workdir(aid))
            .args(args)
            .output()
            .await?;
        let text = output.stdout.into_text()?;
        let elements = vec![Paragraph::new(text).into()];
        let elements = CompositeElement(elements);
        Ok(Self { elements })
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        self.elements.render(area, buf, RenderContext::default());
    }
}
