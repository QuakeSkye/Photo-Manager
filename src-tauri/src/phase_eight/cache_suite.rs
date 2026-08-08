use std::path::Path;

use serde::Serialize;

use crate::{domain::DomainError, safety::PhaseEightRunRoot};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheSuiteReport {
    report_version: u32,
    run_id: uuid::Uuid,
    cache: crate::cache::PhaseEightCacheReport,
}

pub fn run(root: &PhaseEightRunRoot) -> Result<CacheSuiteReport, DomainError> {
    root.verify_current_identity()?;
    let suite = root.resolve_owned(Path::new("candidate-cache-suite-v2"))?;
    let cache = crate::cache::phase_eight_cache_suite(&suite)?;
    root.verify_current_identity()?;
    root.ensure_allocation_within_limit(0)?;
    Ok(CacheSuiteReport {
        report_version: 1,
        run_id: root.run_id(),
        cache,
    })
}
