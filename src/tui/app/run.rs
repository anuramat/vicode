use std::future::pending;

use anyhow::Result;
use crossterm::event::Event;
use futures::future::join_all;
use tokio::time::Duration;
use tokio::time::sleep_until;

use super::App;
use crate::project::PROJECT;
use crate::tui::app::handle::AppEvent;

const MIN_DRAW_INTERVAL: Duration = Duration::from_millis(1000 / 60);

impl<'a> App<'a> {
    pub async fn launch() -> Result<()> {
        Self::new().await?.run().await
    }

    pub async fn run(mut self) -> Result<()> {
        let mut term = ratatui::init();

        // translate key events to app events
        self.spawn_term_translator();
        // first render
        self.draw(&mut term)?;
        // load tabs
        self.load_tabs().await?;
        // TODO cleanup -- delete agents that are not in app state

        let mut render_interval = tokio::time::interval(MIN_DRAW_INTERVAL);
        render_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                // throttled render
                _ = render_interval.tick() => {
                    if self.dirty {
                        self.draw(&mut term)?;
                        self.dirty = false;
                    }
                }

                // notification expiration
                _ = async {
                    if let Some(notification) = self.notification.as_ref() {
                        sleep_until(notification.expires_at).await;
                    } else {
                        pending::<()>().await;
                    }
                } => {
                    self.notification = None;
                    self.dirty = true;
                }

                // handle events
                msg = self.rx.recv() => {
                    self.handle(msg.expect("app event channel closed")).await?;
                    if self.should_exit {
                        self.cleanup().await.expect("failed app clean up");
                        break;
                    }
                    self.dirty = true;
                }
            }
        }
        Ok(())
    }

    async fn cleanup(&self) -> Result<()> {
        PROJECT.save_app_state(self).await?;
        self.reset_osc7();
        let results = join_all(self.agents.keys().map(|i| PROJECT.unmount(i))).await;
        let errors: Vec<String> = results
            .into_iter()
            .filter_map(Result::err)
            .map(|e| e.to_string())
            .collect();
        if errors.is_empty() {
            return Ok(());
        }
        Err(anyhow::anyhow!(
            "multiple errors occured:\n{}",
            errors.join("\n")
        ))
    }

    fn spawn_term_translator(&mut self) {
        use tokio_stream::StreamExt;
        let tx = self.tx.clone();
        self.joinset.spawn(async move {
            let mut stream = crossterm::event::EventStream::new();
            while let Some(Ok(event)) = stream.next().await {
                let e = match event {
                    Event::Key(key) => tx.send(AppEvent::Key(key)),
                    Event::Resize(_, _) => tx.send(AppEvent::Redraw),
                    _ => continue,
                };
                e.await?;
            }
            Ok(())
        });
    }
}
