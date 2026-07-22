use std::collections::BTreeMap;
use std::collections::BTreeSet;
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
use crate::agent::router::graph::GraphRecord;
use crate::tui::app::AppState;

const APP_KEY: &str = "current";
const APP_STATE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("app_state");
const AGENT_STATE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("agent_state");
const AGENT_GRAPH: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("agent_graph");

pub struct Store {
    db: Database,
}

#[derive(Clone)]
pub struct StoreHandle {
    tx: mpsc::UnboundedSender<StoreRequest>,
}

impl std::fmt::Debug for StoreHandle {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.debug_struct("StoreHandle").finish_non_exhaustive()
    }
}

enum StoreRequest {
    Write {
        op: WriteOp,
        done: oneshot::Sender<Result<()>>,
    },
    LoadState {
        aid: AgentId,
        done: oneshot::Sender<Result<AgentState>>,
    },
    LoadGraph {
        done: oneshot::Sender<Result<BTreeMap<AgentId, GraphRecord>>>,
    },
}

enum WriteOp {
    SaveApp(Vec<u8>),
    SaveState(String, Vec<u8>),
    DeleteAgent(String),
    SaveGraph(Vec<(String, Vec<u8>)>),
    DeleteGraph(String),
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            db: Database::create(path)?,
        })
    }

    fn apply(
        &self,
        op: WriteOp,
    ) -> Result<()> {
        let write = self.db.begin_write()?;
        match op {
            WriteOp::SaveApp(data) => {
                let mut table = write.open_table(APP_STATE)?;
                table.insert(APP_KEY, data.as_slice())?;
            }
            WriteOp::SaveState(key, data) => {
                let mut table = write.open_table(AGENT_STATE)?;
                table.insert(key.as_str(), data.as_slice())?;
            }
            WriteOp::DeleteAgent(key) => {
                write.open_table(AGENT_STATE)?.remove(key.as_str())?;
                write.open_table(AGENT_GRAPH)?.remove(key.as_str())?;
            }
            WriteOp::SaveGraph(entries) => {
                let mut table = write.open_table(AGENT_GRAPH)?;
                for (key, data) in entries {
                    table.insert(key.as_str(), data.as_slice())?;
                }
            }
            WriteOp::DeleteGraph(key) => {
                let mut table = write.open_table(AGENT_GRAPH)?;
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

    fn state_table(&self) -> Result<Option<ReadOnlyTable<&'static str, &'static [u8]>>> {
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

    pub fn load_state(
        &self,
        id: &AgentId,
    ) -> Result<AgentState> {
        let table = self.state_table()?.context("no state table yet")?;
        let value = table
            .get(id.to_string().as_str())?
            .with_context(|| format!("agent {id} not found"))?;
        Ok(serde_json::from_slice(value.value())?)
    }

    pub fn state_ids(&self) -> Result<BTreeSet<AgentId>> {
        let mut ids = BTreeSet::new();
        if let Some(table) = self.state_table()? {
            for row in table.iter()? {
                let (key, _) = row?;
                ids.insert(AgentId::from(key.value().to_string()));
            }
        }
        Ok(ids)
    }

    pub fn load_graph(&self) -> Result<BTreeMap<AgentId, GraphRecord>> {
        let read = self.db.begin_read()?;
        let table = match read.open_table(AGENT_GRAPH) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        };
        let mut records = BTreeMap::new();
        for row in table.iter()? {
            let (key, value) = row?;
            records.insert(
                AgentId::from(key.value().to_string()),
                serde_json::from_slice(value.value())?,
            );
        }
        Ok(records)
    }

    pub fn into_handle(self) -> StoreHandle {
        StoreHandle::new(self)
    }
}

impl StoreHandle {
    fn new(store: Store) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        thread::spawn(move || {
            while let Some(cmd) = rx.blocking_recv() {
                match cmd {
                    StoreRequest::Write { op, done } => drop(done.send(store.apply(op))),
                    StoreRequest::LoadState { aid, done } => {
                        drop(done.send(store.load_state(&aid)));
                    }
                    StoreRequest::LoadGraph { done } => drop(done.send(store.load_graph())),
                }
            }
        });
        Self { tx }
    }

    fn write(
        &self,
        op: serde_json::Result<WriteOp>,
    ) -> impl Future<Output = Result<()>> + use<> {
        let (done, rx) = oneshot::channel();
        match op {
            Ok(op) => drop(self.tx.send(StoreRequest::Write { op, done })),
            Err(e) => drop(done.send(Err(e.into()))),
        }
        async move { rx.await.context("state store thread died")? }
    }

    pub fn save_app(
        &self,
        state: &AppState,
    ) -> impl Future<Output = Result<()>> + use<> {
        self.write(serde_json::to_vec(state).map(WriteOp::SaveApp))
    }

    pub fn save_state(
        &self,
        id: &AgentId,
        state: &AgentState,
    ) -> impl Future<Output = Result<()>> + use<> {
        self.write(serde_json::to_vec(state).map(|data| WriteOp::SaveState(id.to_string(), data)))
    }

    pub fn delete_agent(
        &self,
        id: &AgentId,
    ) -> impl Future<Output = Result<()>> + use<> {
        self.write(Ok(WriteOp::DeleteAgent(id.to_string())))
    }

    pub fn delete_graph(
        &self,
        id: &AgentId,
    ) -> impl Future<Output = Result<()>> + use<> {
        self.write(Ok(WriteOp::DeleteGraph(id.to_string())))
    }

    pub fn save_graph(
        &self,
        id: &AgentId,
        record: &GraphRecord,
    ) -> impl Future<Output = Result<()>> + use<> {
        self.save_graph_batch(&[(id.clone(), record.clone())])
    }

    /// one redb transaction for the whole set
    pub fn save_graph_batch(
        &self,
        entries: &[(AgentId, GraphRecord)],
    ) -> impl Future<Output = Result<()>> + use<> {
        let entries = entries
            .iter()
            .map(|(id, r)| Ok((id.to_string(), serde_json::to_vec(r)?)))
            .collect::<serde_json::Result<_>>();
        self.write(entries.map(WriteOp::SaveGraph))
    }

    pub async fn load_state(
        &self,
        id: &AgentId,
    ) -> Result<AgentState> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(StoreRequest::LoadState {
                aid: id.clone(),
                done,
            })
            .ok()
            .context("state store thread died")?;
        rx.await?
    }

    pub async fn load_graph(&self) -> Result<BTreeMap<AgentId, GraphRecord>> {
        let (done, rx) = oneshot::channel();
        self.tx
            .send(StoreRequest::LoadGraph { done })
            .ok()
            .context("state store thread died")?;
        rx.await?
    }
}

#[cfg(test)]
mod tests {
    use similar_asserts::assert_eq;

    use super::*;

    impl Store {
        pub fn save_state_sync(
            &self,
            id: &AgentId,
            state: &AgentState,
        ) -> Result<()> {
            self.apply(WriteOp::SaveState(
                id.to_string(),
                serde_json::to_vec(state)?,
            ))
        }

        pub fn save_graph_sync(
            &self,
            id: &AgentId,
            record: &GraphRecord,
        ) -> Result<()> {
            self.apply(WriteOp::SaveGraph(vec![(
                id.to_string(),
                serde_json::to_vec(record)?,
            )]))
        }
    }

    /// H5: a batch of graph records lands whole in one write
    #[tokio::test]
    async fn save_graph_batch_commits_whole() {
        let dir = std::env::temp_dir().join(format!("vicode-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(dir.join("state.db")).unwrap().into_handle();

        let flips: Vec<(AgentId, GraphRecord)> = ["a", "b", "c"]
            .into_iter()
            .map(|id| {
                (
                    AgentId::from(id.to_string()),
                    GraphRecord {
                        root: AgentId::from("a".to_string()),
                        parent: None,
                        archived: true,
                    },
                )
            })
            .collect();
        store.save_graph_batch(&flips).await.unwrap();

        let records = store.load_graph().await.unwrap();
        assert_eq!(
            records,
            flips
                .into_iter()
                .collect::<BTreeMap<AgentId, GraphRecord>>()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
