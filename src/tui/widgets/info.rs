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
use crate::tui::widgets::container::collapsible_sections::CollapsibleSection;
use crate::tui::widgets::container::collapsible_sections::CollapsibleSections;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::scroll::ScrollOp;

#[derive(Debug, Default)]
pub struct InfoWidget {
    sections: CollapsibleSections,
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

        Ok(Self {
            sections: CollapsibleSections::new([CollapsibleSection::new(
                "status",
                Paragraph::new(output.stdout.into_text()?),
            )]),
        })
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
    ) {
        self.sections.render(area, buf, RenderContext::default());
    }

    pub fn scroll(
        &mut self,
        op: ScrollOp,
    ) {
        self.sections.scroll(op);
    }
}
