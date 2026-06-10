use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::thread;

use anyhow::Context;
use anyhow::Result;
use redb::Database;
use redb::ReadOnlyTable;
use redb::ReadableDatabase;
use redb::ReadableTable;
use redb::TableDefinition;
use redb::TableError;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::agent::AgentState;
use crate::agent::id::AgentId;
use crate::tui::app::AppState;

const APP_KEY: &str = "current";
const APP_STATE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("app_state");
const AGENT_STATE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("agent_state");
const CHANNEL_CAPACITY: usize = 100;

pub struct StateStore {
    db: Database,
}

#[derive(Clone)]
pub struct StateStoreHandle {
    tx: mpsc::Sender<StateCommand>,
}

impl std::fmt::Debug for StateStoreHandle {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.debug_struct("StateStoreHandle").finish_non_exhaustive()
    }
}

struct StateCommand {
    op: StateOp,
    done: oneshot::Sender<Result<()>>,
}

enum StateOp {
    SaveApp(Vec<u8>),
    SaveAgent(String, Vec<u8>),
    DeleteAgent(String),
}

impl StateStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            db: Database::create(path)?,
        })
    }

    fn apply(
        &self,
        op: StateOp,
    ) -> Result<()> {
        let write = self.db.begin_write()?;
        match op {
            StateOp::SaveApp(data) => {
                let mut table = write.open_table(APP_STATE)?;
                table.insert(APP_KEY, data.as_slice())?;
            }
            StateOp::SaveAgent(key, data) => {
                let mut table = write.open_table(AGENT_STATE)?;
                table.insert(key.as_str(), data.as_slice())?;
            }
            StateOp::DeleteAgent(key) => {
                let mut table = write.open_table(AGENT_STATE)?;
                table.remove(key.as_str())?;
            }
        }
        write.commit()?;
        Ok(())
    }

    pub fn load_app(&self) -> Result<AppState> {
        let read = self.db.begin_read()?;
        let table = match read.open_table(APP_STATE) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(AppState::default()),
            Err(e) => return Err(e.into()),
        };
        let Some(value) = table.get(APP_KEY)? else {
            return Ok(AppState::default());
        };
        Ok(serde_json::from_slice(value.value())?)
    }

    fn agent_table(&self) -> Result<Option<ReadOnlyTable<&'static str, &'static [u8]>>> {
        let read = self.db.begin_read()?;
        match read.open_table(AGENT_STATE) {
            Ok(table) => Ok(Some(table)),
            Err(TableError::TableDoesNotExist(_)) => Ok(None),
            Err(e) => {
                let e: anyhow::Error = e.into();
                Err(e.context("failed to open agent state table"))
            }
        }
    }

    pub fn load_agent(
        &self,
        id: &AgentId,
    ) -> Result<AgentState> {
        Ok(serde_json::from_slice(&self.agent_bytes(id)?)?)
    }

    /// commit of an agent without deserializing the full state, which would
    /// require an initialized assistant pool
    pub fn agent_commit(
        &self,
        id: &AgentId,
    ) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct Context {
            commit: String,
        }
        #[derive(serde::Deserialize)]
        struct State {
            context: Context,
        }
        Ok(serde_json::from_slice::<State>(&self.agent_bytes(id)?)?
            .context
            .commit)
    }

    fn agent_bytes(
        &self,
        id: &AgentId,
    ) -> Result<Vec<u8>> {
        let value = self
            .agent_table()?
            .map(|t| t.get(id.to_string().as_str()))
            .transpose()?
            .flatten()
            .with_context(|| format!("agent {id} not found"))?;
        Ok(value.value().to_vec())
    }

    pub fn agent_ids(&self) -> Result<HashSet<AgentId>> {
        let mut agents = HashSet::new();
        if let Some(table) = self.agent_table()? {
            for row in table.iter()? {
                let (key, _) = row?;
                agents.insert(AgentId::from(key.value().to_string()));
            }
        };
        Ok(agents)
    }

    pub fn into_handle(self) -> StateStoreHandle {
        StateStoreHandle::new(self)
    }
}

impl StateStoreHandle {
    fn new(store: StateStore) -> Self {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        thread::spawn(move || {
            while let Some(StateCommand { op, done }) = rx.blocking_recv() {
                drop(done.send(store.apply(op)));
            }
        });
        Self { tx }
    }

    async fn write(
        &self,
        op: StateOp,
    ) -> Result<()> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(StateCommand { op, done })
            .await
            .context("state store channel closed")?;
        rx.await?
    }

    pub async fn save_app(
        &self,
        state: &AppState,
    ) -> Result<()> {
        let data = serde_json::to_vec(state)?;
        self.write(StateOp::SaveApp(data)).await
    }

    pub async fn save_agent(
        &self,
        id: &AgentId,
        state: &AgentState,
    ) -> Result<()> {
        let data = serde_json::to_vec(state)?;
        self.write(StateOp::SaveAgent(id.to_string(), data)).await
    }

    pub async fn delete_agent(
        &self,
        id: &AgentId,
    ) -> Result<()> {
        self.write(StateOp::DeleteAgent(id.to_string())).await
    }
}

#[cfg(test)]
impl StateStore {
    pub fn save_agent_sync(
        &self,
        id: &AgentId,
        state: &AgentState,
    ) -> Result<()> {
        self.apply(StateOp::SaveAgent(
            id.to_string(),
            serde_json::to_vec(state)?,
        ))
    }

    pub fn save_app_sync(
        &self,
        state: &AppState,
    ) -> Result<()> {
        self.apply(StateOp::SaveApp(serde_json::to_vec(state)?))
    }
}
