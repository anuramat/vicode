use std::future::pending;
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::Event;
use crossterm::execute;
use ratatui::DefaultTerminal;
use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::Duration;
use tokio::time::sleep_until;
use tracing_appender::non_blocking::WorkerGuard;

use super::App;
use crate::agent::Agent;
use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::agent::router::Router;
use crate::agent::router::boot::Boot;
use crate::config::Config;
use crate::llm::provider::assistant::AssistantPool;
use crate::project::Paths;
use crate::project::Project;
use crate::project::lock::ProjectLock;
use crate::project::store::Store;
use crate::tui::app::AppEvent;
use crate::tui::app::NotificationKind;
use crate::tui::app::refresh::join_refresh;
use crate::tui::osc7::set_osc7;

const MIN_DRAW_INTERVAL: Duration = Duration::from_millis(1000 / 60);
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

impl App<'_> {
    pub async fn launch(config: Config) -> Result<()> {
        // figure out where we (will) store data etc
        let paths = Paths::new()?;
        // start tracing as early as possible
        let _guard = init_tracing(&paths)?;
        // make sure we're the only instance in this project
        let lock = ProjectLock::acquire(&paths)?;

        let assistants = Arc::new(AssistantPool::from_config(&config).await?);

        // read the tab order, then hand the db to the writer task
        let store = Store::open(paths.state_db())?;
        let visible_order = store.load_app()?.visible_order;
        let project = Project::new(config, paths, lock, store.into_handle(), assistants);
        let (tx, rx) = unbounded_channel();
        let Boot {
            router,
            mut tabs,
            agents,
            failures,
        } = Router::boot(tx.clone(), project.clone()).await?;
        order_tabs(&mut tabs, &visible_order);
        let mut app = Self::with_router(project, tx, rx, router);
        if !failures.is_empty() {
            let list = failures
                .iter()
                .map(|(aid, error)| format!("{aid}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            app.notify(
                NotificationKind::Error,
                format!("failed to restore {list}; data kept on disk"),
            );
        }
        let term = app.setup_terminal()?;
        app.run(term, tabs, agents).await?;
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
        tabs: Vec<(AgentId, AgentState)>,
        agents: Vec<Agent>,
    ) -> Result<()>
    where
        B: Backend,
    {
        // clean up before starting
        self.cleanup().await?;
        // create shared lowerdir
        self.project.init().await?;
        // load tabs
        self.load_tabs(tabs, agents)?;

        tracing::debug!("entering main loop");
        let mut render_interval = tokio::time::interval(MIN_DRAW_INTERVAL);
        render_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut refresh_interval = tokio::time::interval(REFRESH_INTERVAL);
        refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                // throttled render
                _ = render_interval.tick() => {
                    if self.dirty {
                        self.draw(&mut term)?;
                        self.dirty = false;
                    }
                }

                // the selected tab's workdir views; a slow read skips ticks
                _ = refresh_interval.tick() => {
                    if self.refreshing.is_none() {
                        self.refresh();
                    }
                }

                views = join_refresh(&mut self.refreshing) => {
                    self.refreshing = None;
                    if let Err(e) = views.and_then(|views| self.apply_refresh(views)) {
                        self.notify(NotificationKind::Error, e.to_string());
                    }
                    self.dirty = true;
                }

                // notification expiration
                () = async {
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
                    }
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
        // TODO delete unreachable agents
        self.project.unmount_all().await?;
        crate::git::prune_stale_worktrees(&self.project)?;
        Ok(())
    }

    // TODO split into a future with a loop and a function that spawns the task with the future
    fn spawn_crossterm_translator(&self) {
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
                e?;
            }
            Ok::<(), anyhow::Error>(())
        });
    }
}

/// tabs in `visible_order`; a live root missing from it (a crash between
/// the graph write and `save_app`) goes at the end, in boot order. The boot
/// graph is authoritative: an entry with no tab (a crash can leave an
/// archived primary stale in it) is ignored.
fn order_tabs(
    tabs: &mut [(AgentId, AgentState)],
    visible_order: &[AgentId],
) {
    tabs.sort_by_key(|(aid, _)| {
        visible_order
            .iter()
            .position(|a| a == aid)
            .unwrap_or(usize::MAX)
    });
}

pub fn init_tracing(project: &Paths) -> Result<WorkerGuard> {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt;

    use crate::config;

    let dir = config::DIRS.create_state_directory("")?;
    let appender = tracing_appender::rolling::never(&dir, format!("{}.log", project.id()));
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::from_default_env();
    fmt().with_env_filter(filter).with_writer(writer).init();
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    #[test]
    fn order_tabs_follows_visible_order_and_appends_stragglers() {
        let aid = |s: &str| AgentId::from(s.to_string());
        let mut tabs: Vec<_> = ["alive", "missing", "straggler"]
            .into_iter()
            .map(|a| (aid(a), AgentState::fake()))
            .collect();
        // `stale` archived but left in visible_order by a crash; `missing`
        // isn't in it
        let visible = vec![aid("stale"), aid("straggler"), aid("alive")];

        order_tabs(&mut tabs, &visible);

        assert_eq!(
            tabs.into_iter().map(|(a, _)| a).collect::<Vec<_>>(),
            vec![aid("straggler"), aid("alive"), aid("missing")]
        );
    }
}
