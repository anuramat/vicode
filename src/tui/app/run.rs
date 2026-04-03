use std::future::pending;

use anyhow::Result;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::Event;
use crossterm::execute;
use ratatui::DefaultTerminal;
use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::time::Duration;
use tokio::time::sleep_until;

use super::App;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::llm::provider::assistant::AssistantPool;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::tui::app::NotificationKind;
use crate::tui::app::handle::AppEvent;
use crate::tui::osc7::set_osc7;

const MIN_DRAW_INTERVAL: Duration = Duration::from_millis(1000 / 60);

impl<'a> App<'a> {
    pub async fn launch(project: Project) -> Result<()> {
        let mut app = Self::new(project).await?;
        let term = app.setup_terminal()?;
        app.run(term).await?;
        Ok(())
    }

    pub fn setup_terminal(&mut self) -> Result<DefaultTerminal> {
        let mut term = ratatui::init();
        self.draw(&mut term)?; // first render
        tracing::debug!("first render done");
        self.spawn_crossterm_translator();
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        set_osc7(self.project.root());
        Ok(term)
    }

    pub fn reset_terminal() {
        let e = execute!(std::io::stdout(), DisableBracketedPaste);
        if let Err(err) = e {
            tracing::error!("failed to disable braketed paste on exit: {}", err);
        }
        let cwd = std::env::current_dir().unwrap_or_default();
        set_osc7(&cwd);
        ratatui::restore();
    }

    pub async fn run<B>(
        mut self,
        mut term: Terminal<B>,
    ) -> Result<()>
    where
        B: Backend,
    {
        // clean up before starting
        self.cleanup().await?;
        // create shared lowerdir
        self.project.init().await?;
        // load assistants
        ASSISTANT_POOL.get_or_try_init(AssistantPool::new).await?;
        // load tabs
        self.load_tabs().await?;

        tracing::debug!("entering main loop");
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
                    if let Err(e) = self.handle(msg.expect("app event channel closed")).await {
                        self.notify(NotificationKind::Error, e.to_string());
                    };
                    if self.should_exit {
                        self.save_app_state().await?;
                        self.cleanup().await.expect("failed app clean up");
                        break;
                    }
                    self.dirty = true;
                }
            }
        }
        Ok(())
    }

    /// clean up on start / before exit
    async fn cleanup(&self) -> Result<()> {
        // TODO delete unreachable agents, clear stale git worktrees/branches
        self.project.unmount_all().await?;
        Ok(())
    }

    // TODO split into a future with a loop and a function that spawns the task with the future
    fn spawn_crossterm_translator(&mut self) {
        use tokio_stream::StreamExt;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut stream = crossterm::event::EventStream::new();
            while let Some(Ok(event)) = stream.next().await {
                let e = match event {
                    Event::Key(key) => tx.send(AppEvent::Key(key)),
                    Event::Resize(_, _) => tx.send(AppEvent::Redraw),
                    Event::Paste(content) => tx.send(AppEvent::Paste(content)),
                    _ => continue,
                };
                e.await?;
            }
            Ok::<(), anyhow::Error>(())
        });
    }
}
