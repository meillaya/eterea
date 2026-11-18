//! Persisted saved searches over the composable filter spec.
//!
//! Kept in `app_metadata` (no schema change): a JSON list of named
//! [`FilterSpec`]s, deduplicated by case-insensitive name and capped so the file
//! cannot grow without bound.

use super::AppServices;
use crate::types::{FilterSpec, SavedSearch};
use anyhow::{bail, Context, Result};

const SAVED_SEARCHES_KEY: &str = "saved_searches_v1";
const SAVED_SEARCHES_LIMIT: usize = 50;

impl AppServices {
    pub fn saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let Some(raw) = self
            .db
            .get_metadata(SAVED_SEARCHES_KEY)
            .context("failed to read saved searches")?
        else {
            return Ok(Vec::new());
        };
        Ok(serde_json::from_str(&raw).unwrap_or_default())
    }

    /// Insert or replace (case-insensitive name) a saved search, newest first.
    pub fn save_search(&self, name: &str, spec: FilterSpec) -> Result<Vec<SavedSearch>> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a saved search needs a name");
        }
        if spec.is_empty() {
            bail!("an empty filter has nothing to save");
        }
        let mut searches = self.saved_searches()?;
        searches.retain(|search| !search.name.eq_ignore_ascii_case(name));
        searches.insert(
            0,
            SavedSearch {
                name: name.to_string(),
                spec,
            },
        );
        searches.truncate(SAVED_SEARCHES_LIMIT);
        let encoded =
            serde_json::to_string(&searches).context("failed to encode saved searches")?;
        self.db
            .set_metadata(SAVED_SEARCHES_KEY, &encoded)
            .context("failed to store saved searches")?;
        Ok(searches)
    }

    pub fn delete_saved_search(&self, name: &str) -> Result<Vec<SavedSearch>> {
        let mut searches = self.saved_searches()?;
        searches.retain(|search| !search.name.eq_ignore_ascii_case(name.trim()));
        let encoded =
            serde_json::to_string(&searches).context("failed to encode saved searches")?;
        self.db
            .set_metadata(SAVED_SEARCHES_KEY, &encoded)
            .context("failed to store saved searches")?;
        Ok(searches)
    }
}
