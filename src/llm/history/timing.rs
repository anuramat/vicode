use ambassador::delegatable_trait;

pub use crate::utils::now;

/// invariant: `created_at <= started_at <= ended_at <= ready_at`
#[delegatable_trait]
pub trait Timing {
    /// local initialization
    fn created_at(&self) -> u64;
    /// first api event (request sent, first delta, etc)
    fn started_at(&self) -> Option<u64> {
        None
    }
    /// last api event (response finished, last delta, etc)
    fn ended_at(&self) -> Option<u64> {
        None
    }
    /// last local update (tool call result, etc)
    fn ready_at(&self) -> Option<u64> {
        None
    }

    fn duration_str(&self) -> String {
        let start = self.started_at().unwrap_or_else(|| self.created_at());
        let (end, in_progress) = self
            .ready_at()
            .or_else(|| self.ended_at())
            .map_or_else(|| (now(), true), |t| (t, false));

        #[allow(clippy::cast_precision_loss)]
        let ms = end.saturating_sub(start) as f64;
        let s: f64 = ms / 1000_f64;

        if in_progress {
            format!("{s:.1}s+")
        } else {
            format!("{s:.1}s")
        }
    }
}

pub fn touch(
    dest: &mut Option<u64>,
    at_ms: u64,
) {
    if let Some(last) = *dest
        && at_ms < last
    {
        return;
    }
    *dest = Some(at_ms);
}
