use derive_more::Deref;
use derive_more::DerefMut;
use serde::Deserialize;
use serde::Serialize;

use super::state::HistoryState;

#[derive(Clone, Serialize, Deserialize, Debug, Deref, DerefMut)]
pub struct ArchivedHistory {
    #[deref]
    #[deref_mut]
    pub state: HistoryState,
    pub reason: ArchivedHistoryReason,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum ArchivedHistoryReason {
    Compact,
    Undo,
}
