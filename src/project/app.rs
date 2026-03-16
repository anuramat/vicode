use anyhow::Result;

use super::Project;
use crate::tui::app::App;
use crate::tui::app::AppState;

impl Project {
    pub async fn save_app_state(
        &self,
        app: &App<'_>,
    ) -> Result<()> {
        let data = app.state();
        let serialized = serde_json::to_string_pretty(&data)?;
        let path = self.app_state();
        tokio::fs::write(path, serialized).await?;
        Ok(())
    }

    pub async fn load_app_state(&self) -> Result<AppState> {
        let path = self.app_state();
        if !path.exists() {
            return Ok(AppState::default());
        }
        let serialized = tokio::fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&serialized)?)
    }
}
