//! Bounded presentation views. The executable plan remains owned by the facade.
use crate::{AppError, Application, error, io, lock};
use sync_core::ProfileId;

impl Application {
    /// Lists bounded result summaries, without transferring saved plans/outcomes.
    /// # Errors
    /// History persistence or format failure.
    pub fn history_overview(&self, id: ProfileId) -> Result<Vec<crate::Run>, AppError> {
        crate::runs::overview(&self.state, &id)
    }
    /// Reads 100 historical plan/outcome pairs, preserving the full durable model.
    /// # Errors
    /// Missing execution, invalid offset or persistence/format failure.
    pub fn history_page(
        &self,
        id: ProfileId,
        execution: &str,
        offset: usize,
    ) -> Result<serde_json::Value, AppError> {
        if offset > i64::MAX as usize {
            return Err(error("invalid_input", "offset out of range"));
        }
        crate::runs::page(&self.state, &id, execution, offset)
    }
    /// Builds the exact cached plan but sends at most 100 operations over IPC.
    /// # Errors
    /// Same failures as `preview`; no truncation of the executable model.
    pub fn preview_paged(&self, id: ProfileId) -> Result<serde_json::Value, AppError> {
        let _gate = self.gate()?;
        self.preview_locked(id, None, true)
    }
    /// Searches the exact cached plan, returning at most 100 operations.
    /// Stale plans and oversized search values are explicitly refused.
    /// # Errors
    /// Stale plan, invalid query or poisoned application state.
    pub fn plan_page(
        &self,
        id: &str,
        offset: usize,
        query: &str,
        action: &str,
    ) -> Result<serde_json::Value, AppError> {
        if query.len() > 512 || action.len() > 32 {
            return Err(error("invalid_input", "query too large"));
        }
        let cache = lock(&self.cache)?;
        let cached = cache
            .as_ref()
            .filter(|c| c.plan.id().as_str() == id)
            .ok_or_else(|| error("stale", "new preview required"))?;
        let action: Option<sync_plan::Action> = if action.is_empty() {
            None
        } else {
            Some(serde_json::from_value(serde_json::Value::String(action.into())).map_err(io)?)
        };
        let query = query.to_lowercase();
        let matches = || {
            cached.plan.operations().iter().filter(|op| {
                action.is_none_or(|a| op.action() == a)
                    && op.path().as_str().to_lowercase().contains(&query)
            })
        };
        Ok(
            serde_json::json!({"operations":matches().skip(offset).take(100).collect::<Vec<_>>(),"total":matches().count()}),
        )
    }
}
