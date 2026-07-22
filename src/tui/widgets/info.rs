use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use tokio::process::Command;

use crate::agent::id::AgentId;
use crate::deps;
use crate::project::Project;
use crate::tui::widgets::container::collapsible_sections::CollapsibleSection;
use crate::tui::widgets::container::collapsible_sections::CollapsibleSections;
use crate::tui::widgets::container::element::RenderContext;
use crate::tui::widgets::container::scroll::ScrollOp;

#[derive(Debug, Default)]
pub struct InfoWidget {
    sections: CollapsibleSections,
}

/// the stdout of `info_cmd` in the agent's workdir
pub async fn read_info(
    project: &Project,
    aid: &AgentId,
) -> Result<Vec<u8>> {
    let args = vec!["-c".to_string(), project.config().info_cmd.clone()];
    let output = Command::new(deps::BASH)
        .current_dir(project.agent_workdir(aid))
        .args(args)
        .output()
        .await?;
    Ok(output.stdout)
}

impl InfoWidget {
    pub fn new(stdout: &[u8]) -> Result<Self> {
        Ok(Self {
            sections: CollapsibleSections::new([CollapsibleSection::new(
                "status",
                Paragraph::new(stdout.into_text()?),
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
