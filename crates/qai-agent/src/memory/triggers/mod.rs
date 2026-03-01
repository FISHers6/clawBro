pub mod cron_result;
pub mod idle_distill;
pub mod nightly;
pub mod nturn_distill;
pub mod user_remember;

pub use cron_result::CronResultTrigger;
pub use idle_distill::IdleDistillTrigger;
pub use nightly::NightlyConsolidationTrigger;
pub use nturn_distill::NTurnDistillTrigger;
pub use user_remember::UserRememberTrigger;

use crate::memory::MemoryTrigger;
use std::sync::Arc;

/// Build the default set of built-in triggers.
pub fn default_triggers(distill_every_n: u64) -> Vec<Arc<dyn MemoryTrigger>> {
    vec![
        Arc::new(UserRememberTrigger),
        Arc::new(NTurnDistillTrigger::new(distill_every_n)),
        Arc::new(IdleDistillTrigger),
        Arc::new(CronResultTrigger),
        Arc::new(NightlyConsolidationTrigger),
    ]
}
