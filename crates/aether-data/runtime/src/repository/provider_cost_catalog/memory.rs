use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    ProviderCostCatalogDeleteOutcome, ProviderCostCatalogListQuery, ProviderCostCatalogRecord,
    ProviderCostCatalogRepository, ProviderCostCatalogUpsertOutcome, ProviderCostTaskType,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryProviderCostCatalogRepository {
    items: RwLock<Vec<ProviderCostCatalogRecord>>,
}

impl InMemoryProviderCostCatalogRepository {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = ProviderCostCatalogRecord>,
    {
        Self {
            items: RwLock::new(items.into_iter().collect()),
        }
    }

    fn with_items<T>(
        &self,
        f: impl FnOnce(&Vec<ProviderCostCatalogRecord>) -> T,
    ) -> Result<T, DataLayerError> {
        let items = self
            .items
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("provider cost catalog lock".into()))?;
        Ok(f(&items))
    }

    fn with_items_mut<T>(
        &self,
        f: impl FnOnce(&mut Vec<ProviderCostCatalogRecord>) -> Result<T, DataLayerError>,
    ) -> Result<T, DataLayerError> {
        let mut items = self
            .items
            .write()
            .map_err(|_| DataLayerError::UnexpectedValue("provider cost catalog lock".into()))?;
        f(&mut items)
    }
}

#[async_trait]
impl ProviderCostCatalogRepository for InMemoryProviderCostCatalogRepository {
    async fn upsert_provider_cost_catalog(
        &self,
        record: ProviderCostCatalogRecord,
    ) -> Result<ProviderCostCatalogUpsertOutcome, DataLayerError> {
        record.validate()?;
        self.with_items_mut(|items| {
            if let Some(existing) = items.iter_mut().find(|item| item.cost_id == record.cost_id) {
                *existing = record;
                Ok(ProviderCostCatalogUpsertOutcome::Updated)
            } else {
                items.push(record);
                Ok(ProviderCostCatalogUpsertOutcome::Inserted)
            }
        })
    }

    async fn get_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<Option<ProviderCostCatalogRecord>, DataLayerError> {
        self.with_items(|items| items.iter().find(|item| item.cost_id == cost_id).cloned())
    }

    async fn list_provider_cost_catalogs(
        &self,
        query: &ProviderCostCatalogListQuery,
    ) -> Result<Vec<ProviderCostCatalogRecord>, DataLayerError> {
        self.with_items(|items| {
            let mut filtered: Vec<ProviderCostCatalogRecord> = items
                .iter()
                .filter(|item| {
                    query
                        .provider_id
                        .as_deref()
                        .is_none_or(|provider_id| item.provider_id == provider_id)
                        && query
                            .model
                            .as_deref()
                            .is_none_or(|model| item.model == model)
                        && query
                            .task_type
                            .is_none_or(|task_type| item.task_type == task_type)
                        && query.effective_at_unix_secs.is_none_or(|at| {
                            item.effective_from_unix_secs <= at
                                && item.effective_to_unix_secs.is_none_or(|to| at < to)
                        })
                })
                .cloned()
                .collect();
            filtered.sort_by(|left, right| {
                left.provider_id
                    .cmp(&right.provider_id)
                    .then_with(|| left.model.cmp(&right.model))
                    .then_with(|| {
                        left.effective_from_unix_secs
                            .cmp(&right.effective_from_unix_secs)
                    })
                    .then_with(|| left.cost_id.cmp(&right.cost_id))
            });
            filtered
                .into_iter()
                .skip(query.offset)
                .take(query.limit)
                .collect()
        })
    }

    async fn find_effective_provider_cost_catalog(
        &self,
        provider_id: &str,
        model: &str,
        task_type: ProviderCostTaskType,
        at_unix_secs: u64,
    ) -> Result<Option<ProviderCostCatalogRecord>, DataLayerError> {
        self.with_items(|items| {
            items
                .iter()
                .filter(|item| {
                    item.provider_id == provider_id
                        && item.model == model
                        && item.task_type == task_type
                        && item.effective_from_unix_secs <= at_unix_secs
                        && item
                            .effective_to_unix_secs
                            .is_none_or(|to| at_unix_secs < to)
                })
                .max_by_key(|item| item.effective_from_unix_secs)
                .cloned()
        })
    }

    async fn delete_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<ProviderCostCatalogDeleteOutcome, DataLayerError> {
        self.with_items_mut(|items| {
            let before = items.len();
            items.retain(|item| item.cost_id != cost_id);
            if items.len() == before {
                Ok(ProviderCostCatalogDeleteOutcome::NotFound)
            } else {
                Ok(ProviderCostCatalogDeleteOutcome::Deleted)
            }
        })
    }
}
