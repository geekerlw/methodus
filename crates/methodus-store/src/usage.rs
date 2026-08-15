//! Executor usage rolls: one row per Result event that reported tokens or cost.

use chrono::{DateTime, Utc};
use rusqlite::params;
use uuid::Uuid;

use methodus_domain::{UsageDelta, UsageSummary};

use crate::{Store, StoreError};

impl Store {
    pub fn insert_usage(
        &self,
        task_id: Option<&str>,
        session_id: Option<&str>,
        runtime: Option<&str>,
        delta: &UsageDelta,
    ) -> Result<(), StoreError> {
        if delta.is_empty() {
            return Ok(());
        }
        let id = format!("use_{}", Uuid::new_v4().as_simple());
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO usage_rolls
             (id, task_id, session_id, runtime, input_tokens, output_tokens,
              cache_read_tokens, cache_write_tokens, cost_usd, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                task_id,
                session_id,
                runtime,
                delta.input_tokens,
                delta.output_tokens,
                delta.cache_read_tokens,
                delta.cache_write_tokens,
                delta.cost_usd,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn usage_summary(&self, since: Option<DateTime<Utc>>) -> Result<UsageSummary, StoreError> {
        let conn = self.lock_conn()?;
        let row = if let Some(since) = since {
            conn.query_row(
                "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                        COALESCE(SUM(cost_usd),0), COUNT(*)
                 FROM usage_rolls WHERE occurred_at >= ?1",
                params![since.to_rfc3339()],
                usage_row,
            )?
        } else {
            conn.query_row(
                "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                        COALESCE(SUM(cost_usd),0), COUNT(*)
                 FROM usage_rolls",
                [],
                usage_row,
            )?
        };
        Ok(row)
    }

    pub fn usage_for_task(&self, task_id: &str) -> Result<UsageSummary, StoreError> {
        let conn = self.lock_conn()?;
        let row = conn.query_row(
            "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                    COALESCE(SUM(cost_usd),0), COUNT(*)
             FROM usage_rolls WHERE task_id = ?1",
            params![task_id],
            usage_row,
        )?;
        Ok(row)
    }
}

fn usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageSummary> {
    Ok(UsageSummary {
        input_tokens: row.get(0)?,
        output_tokens: row.get(1)?,
        cache_read_tokens: row.get(2)?,
        cache_write_tokens: row.get(3)?,
        cost_usd: row.get(4)?,
        turns: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[test]
    fn rolls_sum() {
        let store = Store::open_memory().unwrap();
        let d = UsageDelta {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 50,
            cache_write_tokens: 0,
            cost_usd: Some(0.02),
        };
        store
            .insert_usage(Some("t1"), Some("s1"), Some("claude-code"), &d)
            .unwrap();
        store
            .insert_usage(
                Some("t1"),
                Some("s2"),
                Some("claude-code"),
                &UsageDelta {
                    input_tokens: 10,
                    output_tokens: 5,
                    cost_usd: Some(0.01),
                    ..UsageDelta::default()
                },
            )
            .unwrap();
        let all = store.usage_summary(None).unwrap();
        assert_eq!(all.input_tokens, 110);
        assert_eq!(all.output_tokens, 25);
        assert_eq!(all.cache_read_tokens, 50);
        assert!((all.cost_usd - 0.03).abs() < 1e-9);
        assert_eq!(all.turns, 2);
        let task = store.usage_for_task("t1").unwrap();
        assert_eq!(task.turns, 2);
        store
            .insert_usage(None, None, None, &UsageDelta::default())
            .unwrap();
        assert_eq!(store.usage_summary(None).unwrap().turns, 2);
    }
}
