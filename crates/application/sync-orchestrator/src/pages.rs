//! Bounded presentation views. The executable plan remains owned by the facade.
use crate::{AppError, Application, DeletionCeiling, error, io, lock};
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
        self.history_page_filtered(id, execution, offset, crate::HistoryFilter::All, "")
    }
    /// Searches paths and stored diagnostics, and filters outcomes before
    /// selecting a page of at most 100 original plan/outcome pairs.
    /// # Errors
    /// Missing execution, query over 512 bytes, invalid offset or persistence failure.
    pub fn history_page_filtered(
        &self,
        id: ProfileId,
        execution: &str,
        offset: usize,
        filter: crate::HistoryFilter,
        query: &str,
    ) -> Result<serde_json::Value, AppError> {
        crate::history::page(&self.state, &id, execution, offset, filter, query)
    }
    /// One folder level of a historical run, with the filter and search of
    /// [`Self::history_page_filtered`]: sub-folders with their counts, then at
    /// most [`crate::TREE_PAGE`] plan/outcome pairs directly inside `folder`
    /// (`""` for the root), from `offset`. The folder counts are per action;
    /// historical entries carry no byte total.
    /// # Errors
    /// Missing execution, invalid folder or query, or persistence failure.
    pub fn history_tree(
        &self,
        id: ProfileId,
        execution: &str,
        folder: &str,
        offset: usize,
        filter: crate::HistoryFilter,
        query: &str,
    ) -> Result<serde_json::Value, AppError> {
        crate::history::tree(&self.state, &id, execution, folder, offset, filter, query)
    }
    /// Builds the exact cached plan but sends at most 100 operations over IPC.
    /// # Errors
    /// Same failures as `preview`; no truncation of the executable model.
    pub fn preview_paged(&self, id: ProfileId) -> Result<serde_json::Value, AppError> {
        self.preview_paged_with(id, DeletionCeiling::Guarded)
    }
    /// Same as `preview_paged`, with the deletion ceiling chosen by the user
    /// for this preview only (see [`DeletionCeiling`]).
    /// # Errors
    /// Same failures as `preview`.
    pub fn preview_paged_with(
        &self,
        id: ProfileId,
        ceiling: DeletionCeiling,
    ) -> Result<serde_json::Value, AppError> {
        let _gate = self.gate()?;
        self.preview_locked(id, None, true, ceiling)
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
                // No per-path lowercase copy when there is nothing to search.
                action.is_none_or(|a| op.action() == a)
                    && (query.is_empty() || op.path().as_str().to_lowercase().contains(&query))
            })
        };
        Ok(
            serde_json::json!({"operations":matches().skip(offset).take(100).collect::<Vec<_>>(),"total":matches().count()}),
        )
    }
    /// One folder level of the exact cached plan, with the same search and
    /// action filter as [`Self::plan_page`]: sub-folders with their counts
    /// per action and bytes, then at most [`crate::TREE_PAGE`] operations
    /// directly inside `folder` (`""` for the root), from `offset`.
    /// `{"folders":[TreeFolder],"operations":[…],"total":n}`.
    /// # Errors
    /// Stale plan, invalid folder, query or action.
    pub fn plan_tree(
        &self,
        id: &str,
        folder: &str,
        offset: usize,
        query: &str,
        action: &str,
    ) -> Result<serde_json::Value, AppError> {
        crate::tree::check(folder, offset)?;
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
        let operations = cached.plan.operations();
        let codes: Vec<String> = operations
            .iter()
            .map(|op| serde_json::to_value(op.action()).map_err(io))
            .map(|v| v.map(|v| v.as_str().unwrap_or_default().to_owned()))
            .collect::<Result<_, _>>()?;
        let level = crate::tree::level(
            operations
                .iter()
                .zip(&codes)
                .enumerate()
                .filter(|(_, (op, _))| {
                    action.is_none_or(|a| op.action() == a)
                        && (query.is_empty() || op.path().as_str().to_lowercase().contains(&query))
                })
                .map(|(index, (op, code))| {
                    (
                        index,
                        crate::tree::Item {
                            path: op.path().as_str(),
                            action: code,
                            bytes: op.bytes().unwrap_or(0),
                        },
                    )
                }),
            folder,
            offset,
        );
        let page: Vec<_> = level
            .entries
            .iter()
            .filter_map(|index| operations.get(*index))
            .collect();
        Ok(serde_json::json!({"folders":level.folders,"operations":page,"total":level.total}))
    }
}
