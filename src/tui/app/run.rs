use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
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
use tokio::sync::mpsc::channel;
use tokio::time::Duration;
use tokio::time::sleep_until;
use tracing_appender::non_blocking::WorkerGuard;

use super::App;
use super::CHANNEL_CAPACITY;
use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::agent::router::AgentRouter;
use crate::agent::router::api::TurnOutcome;
use crate::agent::router::graph::GraphRecord;
use crate::config::Config;
use crate::llm::provider::assistant::AssistantPool;
use crate::project::Paths;
use crate::project::Project;
use crate::project::lock::ProjectLock;
use crate::project::store::Store;
use crate::tui::app::AppEvent;
use crate::tui::app::NotificationKind;
use crate::tui::osc7::set_osc7;

const MIN_DRAW_INTERVAL: Duration = Duration::from_millis(1000 / 60);

impl App<'_> {
    pub async fn launch(config: Config) -> Result<()> {
        // figure out where we (will) store data etc
        let paths = Paths::new()?;
        // start tracing as early as possible
        let _guard = init_tracing(&paths)?;
        // make sure we're the only instance in this project
        let lock = ProjectLock::acquire(&paths)?;

        let assistants = Arc::new(AssistantPool::from_config(&config).await?);

        // read everything we need at startup, then hand the db to the writer task
        let store = Store::open(paths.state_db())?;
        let app_state = store.load_app()?;
        let state_ids = store.state_ids()?;
        let records = store.load_graph()?;
        let boot = load_boot_agents(&store, &app_state.visible_order, &records);
        let project = Project::new(config, paths, lock, store.into_handle(), assistants);
        let (tx, rx) = channel(CHANNEL_CAPACITY);
        let router = AgentRouter::spawn(
            tx.clone(),
            project.clone(),
            records,
            state_ids,
            boot.outcomes,
        );
        let mut app = Self::with_router(project, tx, rx, router);
        if !boot.failures.is_empty() {
            let list = boot
                .failures
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
        app.run(term, boot.tabs, boot.agents).await?;
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
        agents: Vec<(AgentId, AgentState)>,
    ) -> Result<()>
    where
        B: Backend,
    {
        // clean up before starting
        self.cleanup().await?;
        // create shared lowerdir
        self.project.init().await?;
        // load tabs
        self.load_tabs(tabs, agents).await?;

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

struct BootAgents {
    tabs: Vec<(AgentId, AgentState)>,
    agents: Vec<(AgentId, AgentState)>,
    outcomes: HashMap<AgentId, TurnOutcome>,
    failures: Vec<(AgentId, String)>,
}

/// Load boot state while the database is still synchronous: roots first, then
/// children under valid roots — state under an invalid root is never read
/// (their root's failure notification covers them). Invalid roots omit their
/// whole tab; invalid children remain reachable as terminal `Dead` nodes,
/// without preventing valid descendants from starting.
fn load_boot_agents(
    store: &Store,
    visible_order: &[AgentId],
    records: &BTreeMap<AgentId, GraphRecord>,
) -> BootAgents {
    let load = |aid: &AgentId| {
        let state = store.load_state(aid).map_err(|error| {
            tracing::error!("failed to restore agent {aid}: {error:?}");
            format!("{error:#}")
        });
        (aid.clone(), state)
    };
    let roots = boot_order(visible_order, records);
    let mut loaded: HashMap<AgentId, Result<AgentState, String>> = roots.iter().map(load).collect();
    let valid_roots: HashSet<AgentId> = roots
        .iter()
        .filter(|aid| loaded[*aid].is_ok())
        .cloned()
        .collect();
    let mut children: Vec<AgentId> = records
        .iter()
        .filter(|(_, record)| {
            !record.archived
                && record.parent.is_some()
                && valid_roots.contains(&record.root)
        })
        .map(|(aid, _)| aid.clone())
        .collect();
    children.sort();
    loaded.extend(children.iter().map(load));

    let mut outcomes = HashMap::new();
    let mut failures = Vec::new();
    for aid in roots.iter().chain(children.iter()) {
        match &loaded[aid] {
            Ok(state) => {
                outcomes.insert(
                    aid.clone(),
                    TurnOutcome {
                        output: state.context.history.state().last_text_output().ok(),
                        error: None,
                    },
                );
            }
            Err(error) => {
                failures.push((aid.clone(), error.clone()));
                // Invalid roots and their tabs are omitted entirely.
                if records[aid].parent.is_some() {
                    outcomes.insert(
                        aid.clone(),
                        TurnOutcome {
                            output: None,
                            error: Some(error.clone()),
                        },
                    );
                }
            }
        }
    }
    let mut take = |aid: &AgentId| {
        loaded
            .remove(aid)
            .and_then(Result::ok)
            .map(|state| (aid.clone(), state))
    };
    let tabs: Vec<(AgentId, AgentState)> = roots.iter().filter_map(&mut take).collect();
    let mut agents = tabs.clone();
    agents.extend(children.iter().filter_map(take));
    BootAgents {
        tabs,
        agents,
        outcomes,
        failures,
    }
}

/// the tabs to restore, in order: the graph table's archived flag is
/// authoritative over `visible_order` (a crash can leave an archived primary
/// stale in it), and a live primary missing from `visible_order` (a
/// crash between the graph write and `save_app`) is appended at the end.
fn boot_order(
    visible_order: &[AgentId],
    records: &BTreeMap<AgentId, GraphRecord>,
) -> Vec<AgentId> {
    let restorable = |aid: &AgentId| {
        records
            .get(aid)
            .is_some_and(|r| !r.archived && r.parent.is_none())
    };
    let mut order: Vec<AgentId> = visible_order
        .iter()
        .filter(|a| restorable(a))
        .cloned()
        .collect();
    let stragglers: Vec<AgentId> = records
        .keys()
        .filter(|a| restorable(a) && !order.contains(a))
        .cloned()
        .collect();
    order.extend(stragglers);
    order
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
    use crate::agent::router::graph::GraphRecord;

    #[test]
    fn boot_order_skips_archived_and_appends_stragglers() {
        let aid = |s: &str| AgentId::from(s.to_string());
        let record = |root: &str, parent: Option<&str>, archived| GraphRecord {
            root: aid(root),
            parent: parent.map(aid),
            archived,
        };
        let records = [
            ("alive", record("alive", None, false)),
            ("stale", record("stale", None, true)),
            ("straggler", record("straggler", None, false)),
            ("rowless", record("rowless", None, false)),
            ("sub", record("alive", Some("alive"), false)),
        ]
        .into_iter()
        .map(|(k, v)| (aid(k), v))
        .collect();
        // `stale` archived but left in visible_order by a crash; `straggler`
        // and stateless alive roots are missing from it; `recordless` predates
        // the graph table
        let visible = vec![aid("stale"), aid("alive"), aid("recordless")];

        assert_eq!(
            boot_order(&visible, &records),
            vec![aid("alive"), aid("rowless"), aid("straggler")]
        );
    }

    /// Partial boot: an invalid root is omitted, an invalid child is retained
    /// as Dead, and its valid descendant still starts. All state records stay
    /// on disk.
    #[test]
    fn partial_restore_isolated_per_agent() {
        let aid = |s: &str| AgentId::from(s.to_string());
        let dir = std::env::temp_dir().join(format!("vicode-boot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths {
            id: Paths::derive_id(&dir),
            root: dir.clone(),
            data: dir.clone(),
        };
        let path = paths.state_db();
        {
            let table = redb::TableDefinition::<&str, &[u8]>::new("agent_state");
            let db = redb::Database::create(&path).unwrap();
            let write = db.begin_write().unwrap();
            {
                let mut t = write.open_table(table).unwrap();
                let good = serde_json::to_vec(&AgentState::fake()).unwrap();
                for id in ["good", "grandchild", "under-bad-root", "archived"] {
                    t.insert(id, good.as_slice()).unwrap();
                }
                for id in ["bad-root", "bad-child"] {
                    t.insert(id, b"not json".as_slice()).unwrap();
                }
            }
            write.commit().unwrap();
        }

        let store = Store::open(&path).unwrap();
        let record = |root: &str, parent: Option<&str>, archived| GraphRecord {
            root: aid(root),
            parent: parent.map(aid),
            archived,
        };
        let records = [
            ("good", record("good", None, false)),
            ("bad-root", record("bad-root", None, false)),
            (
                "bad-child",
                record("good", Some("good"), false),
            ),
            (
                "grandchild",
                record("good", Some("bad-child"), false),
            ),
            (
                "under-bad-root",
                record("bad-root", Some("bad-root"), false),
            ),
            ("archived", record("archived", None, true)),
        ]
        .into_iter()
        .map(|(id, record)| (aid(id), record))
        .collect();
        let boot = load_boot_agents(
            &store,
            &[aid("bad-root"), aid("good"), aid("archived")],
            &records,
        );

        assert_eq!(
            boot.tabs.iter().map(|(a, _)| a.clone()).collect::<Vec<_>>(),
            vec![aid("good")]
        );
        assert_eq!(
            boot.agents
                .iter()
                .map(|(a, _)| a.clone())
                .collect::<Vec<_>>(),
            vec![aid("good"), aid("grandchild")]
        );
        assert_eq!(
            boot.failures
                .iter()
                .map(|(a, _)| a.clone())
                .collect::<Vec<_>>(),
            vec![aid("bad-root"), aid("bad-child")]
        );
        assert!(boot.outcomes[&aid("bad-child")].error.is_some());
        assert!(!boot.outcomes.contains_key(&aid("bad-root")));
        assert!(!boot.outcomes.contains_key(&aid("under-bad-root")));
        assert!(store.state_ids().unwrap().contains(&aid("bad-child")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
