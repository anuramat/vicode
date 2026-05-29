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
use tracing_appender::non_blocking::WorkerGuard;

use super::App;
use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::config::Config;
use crate::llm::provider::assistant::ASSISTANT_POOL;
use crate::llm::provider::assistant::AssistantPool;
use crate::project::Layout;
use crate::project::Project;
use crate::project::layout::LayoutTrait;
use crate::project::lock::ProjectLock;
use crate::project::state::StateStore;
use crate::tui::app::AppEvent;
use crate::tui::app::NotificationKind;
use crate::tui::osc7::set_osc7;

const MIN_DRAW_INTERVAL: Duration = Duration::from_millis(1000 / 60);

impl App<'_> {
    pub async fn launch(config: Config) -> Result<()> {
        // figure out where we (will) store data etc
        let layout = Layout::discover()?;
        // start tracing as early as possible
        let _guard = init_tracing(&layout)?;
        // make sure we're the only instance in this project
        let lock = ProjectLock::acquire(&layout)?;

        // TODO move to AgentRouter?
        ASSISTANT_POOL
            .get_or_try_init(|| AssistantPool::from_config(&config))
            .await?;

        // read everything we need at startup, then hand the db to the writer task
        let store = StateStore::open(layout.state_db())?;
        let app_state = store.load_app()?;
        let agent_ids = store.agent_ids()?;
        let tab_agents = {
            let agents: Result<Vec<(AgentId, AgentState)>> = app_state
                .visible_order
                .iter()
                .filter(|aid| agent_ids.contains(aid))
                .map(|aid| store.load_agent(aid).map(|state| (aid.clone(), state)))
                .collect();
            agents?
        };
        let project = Project::new(config, layout, lock, store.into_handle());
        let mut app = Self::new(project, agent_ids);
        let term = app.setup_terminal()?;
        app.run(term, tab_agents).await?;
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
        tab_agents: Vec<(AgentId, AgentState)>,
    ) -> Result<()>
    where
        B: Backend,
    {
        // clean up before starting
        self.cleanup().await?;
        // create shared lowerdir
        self.project.init().await?;
        // load tabs
        self.load_tabs(tab_agents).await?;

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
                e.await?;
            }
            Ok::<(), anyhow::Error>(())
        });
    }
}

pub fn init_tracing(project: &impl LayoutTrait) -> Result<WorkerGuard> {
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
