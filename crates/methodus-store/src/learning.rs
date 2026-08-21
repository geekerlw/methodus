//! SQLite repositories for continuous-learning goals, attention, and budget.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};

use methodus_domain::{
    AttentionKind, AttentionStatus, Cadence, GoalRun, GoalUsage, HumanAttention, LearningGoal,
    QuietHours, ReviewPolicy, WorkKind,
};

use crate::{Store, StoreError};

const GOAL_COLUMNS: &str = "id,title,prompt,sources,runtime,permission_mode,cadence,review_cadence,\
summary_cadence,source_check_cadence,quiet_hours_start,quiet_hours_end,budget_usd,review_policy,\
enabled,next_run_at,next_review_at,next_summary_at,next_source_check_at,created_at,updated_at";

const ATTENTION_COLUMNS: &str = "id,run_id,goal_id,kind,title,prompt,context,tool_name,tool_input,\
status,created_at,resolved_at,response";

impl Store {
    // ─── Goals ───────────────────────────────────────────────────────────

    pub fn upsert_learning_goal(&self, goal: &LearningGoal) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO learning_goals
             (id,title,prompt,sources,runtime,permission_mode,cadence,review_cadence,
              summary_cadence,source_check_cadence,quiet_hours_start,quiet_hours_end,budget_usd,
              review_policy,enabled,next_run_at,next_review_at,next_summary_at,
              next_source_check_at,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)
             ON CONFLICT(id) DO UPDATE SET
             title=excluded.title,prompt=excluded.prompt,sources=excluded.sources,
             runtime=excluded.runtime,permission_mode=excluded.permission_mode,
             cadence=excluded.cadence,review_cadence=excluded.review_cadence,
             summary_cadence=excluded.summary_cadence,
             source_check_cadence=excluded.source_check_cadence,
             quiet_hours_start=excluded.quiet_hours_start,quiet_hours_end=excluded.quiet_hours_end,
             budget_usd=excluded.budget_usd,review_policy=excluded.review_policy,
             enabled=excluded.enabled,next_run_at=excluded.next_run_at,
             next_review_at=excluded.next_review_at,next_summary_at=excluded.next_summary_at,
             next_source_check_at=excluded.next_source_check_at,updated_at=excluded.updated_at",
            params![
                goal.id,
                goal.title,
                goal.prompt,
                serde_json::to_string(&goal.sources).unwrap_or_else(|_| "[]".into()),
                goal.runtime,
                goal.permission_mode,
                goal.cadence.to_string(),
                goal.review_cadence.to_string(),
                goal.summary_cadence.to_string(),
                goal.source_check_cadence.to_string(),
                goal.quiet_hours.map(|window| window.start.format("%H:%M").to_string()),
                goal.quiet_hours.map(|window| window.end.format("%H:%M").to_string()),
                goal.budget_usd,
                goal.review_policy.as_str(),
                goal.enabled as i64,
                goal.next_run_at.map(|at| at.to_rfc3339()),
                goal.next_review_at.map(|at| at.to_rfc3339()),
                goal.next_summary_at.map(|at| at.to_rfc3339()),
                goal.next_source_check_at.map(|at| at.to_rfc3339()),
                goal.created_at.to_rfc3339(),
                goal.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn learning_goal(&self, id: &str) -> Result<Option<LearningGoal>, StoreError> {
        let conn = self.lock_conn()?;
        conn.query_row(
            &format!("SELECT {GOAL_COLUMNS} FROM learning_goals WHERE id = ?1"),
            [id],
            goal_from_row,
        )
        .optional()
        .map_err(StoreError::from)
    }

    pub fn list_learning_goals(&self) -> Result<Vec<LearningGoal>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {GOAL_COLUMNS} FROM learning_goals
             ORDER BY enabled DESC, title COLLATE NOCASE"
        ))?;
        let rows = stmt.query_map([], goal_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    pub fn delete_learning_goal(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let removed = conn.execute("DELETE FROM learning_goals WHERE id = ?1", [id])?;
        Ok(removed > 0)
    }

    // ─── Human attention ─────────────────────────────────────────────────

    pub fn insert_attention(&self, attention: &HumanAttention) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO learning_attentions
             (id,run_id,goal_id,kind,title,prompt,context,tool_name,tool_input,status,
              created_at,resolved_at,response)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                attention.id,
                attention.run_id,
                attention.goal_id,
                attention.kind.as_str(),
                attention.title,
                attention.prompt,
                attention.context,
                attention.tool_name,
                attention.tool_input,
                attention.status.as_str(),
                attention.created_at.to_rfc3339(),
                attention.resolved_at.map(|at| at.to_rfc3339()),
                attention.response,
            ],
        )?;
        Ok(())
    }

    /// Resolve an open item. Returns `false` when it was already resolved, so a
    /// double answer from two surfaces cannot be applied twice.
    pub fn resolve_attention(
        &self,
        id: &str,
        response: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let updated = conn.execute(
            "UPDATE learning_attentions
             SET status = 'resolved', response = ?2, resolved_at = ?3
             WHERE id = ?1 AND status = 'open'",
            params![id, response, at.to_rfc3339()],
        )?;
        Ok(updated > 0)
    }

    pub fn list_open_attentions(&self) -> Result<Vec<HumanAttention>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {ATTENTION_COLUMNS} FROM learning_attentions
             WHERE status = 'open' ORDER BY created_at"
        ))?;
        let rows = stmt.query_map([], attention_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    pub fn list_attentions_for_run(&self, run_id: &str) -> Result<Vec<HumanAttention>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {ATTENTION_COLUMNS} FROM learning_attentions
             WHERE run_id = ?1 ORDER BY created_at"
        ))?;
        let rows = stmt.query_map([run_id], attention_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }

    // ─── Budget ──────────────────────────────────────────────────────────

    /// Add `delta_usd` to a Goal's spend for `month` and return the new total.
    ///
    /// The addition happens inside SQLite so concurrent turns cannot lose a
    /// charge the way a read-modify-write of a JSON file could.
    pub fn record_goal_spend(
        &self,
        goal_id: &str,
        month: &str,
        delta_usd: f64,
        at: DateTime<Utc>,
    ) -> Result<f64, StoreError> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "INSERT INTO goal_usage (goal_id, month, spent_usd, updated_at)
             VALUES (?1,?2,?3,?4)
             ON CONFLICT(goal_id, month) DO UPDATE SET
             spent_usd = spent_usd + excluded.spent_usd, updated_at = excluded.updated_at
             RETURNING spent_usd",
            params![goal_id, month, delta_usd, at.to_rfc3339()],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
    }

    pub fn goal_usage(&self, goal_id: &str, month: &str) -> Result<Option<GoalUsage>, StoreError> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT goal_id,month,spent_usd,updated_at FROM goal_usage
             WHERE goal_id = ?1 AND month = ?2",
            params![goal_id, month],
            |row| {
                Ok(GoalUsage {
                    goal_id: row.get(0)?,
                    month: row.get(1)?,
                    spent_usd: row.get(2)?,
                    updated_at: parse_time(row.get(3)?)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
    }

    // ─── Run links ───────────────────────────────────────────────────────

    pub fn link_goal_run(&self, link: &GoalRun) -> Result<(), StoreError> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO goal_runs (run_id, goal_id, work, created_at) VALUES (?1,?2,?3,?4)
             ON CONFLICT(run_id) DO UPDATE SET goal_id=excluded.goal_id, work=excluded.work",
            params![
                link.run_id,
                link.goal_id,
                link.work.as_str(),
                link.created_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn goal_run(&self, run_id: &str) -> Result<Option<GoalRun>, StoreError> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT run_id,goal_id,work,created_at FROM goal_runs WHERE run_id = ?1",
            [run_id],
            goal_run_from_row,
        )
        .optional()
        .map_err(StoreError::from)
    }

    pub fn list_goal_runs(&self, goal_id: &str, limit: usize) -> Result<Vec<GoalRun>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT run_id,goal_id,work,created_at FROM goal_runs
             WHERE goal_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![goal_id, limit as i64], goal_run_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
    }
}

// ─── Row mapping ─────────────────────────────────────────────────────────────

fn goal_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LearningGoal> {
    let quiet_start: Option<String> = row.get(10)?;
    let quiet_end: Option<String> = row.get(11)?;
    let quiet_hours = match (quiet_start, quiet_end) {
        (Some(start), Some(end)) => Some(
            QuietHours::parse(&start, &end)
                .map_err(|err| conversion(10, Box::new(err)))?,
        ),
        _ => None,
    };
    Ok(LearningGoal {
        id: row.get(0)?,
        title: row.get(1)?,
        prompt: row.get(2)?,
        sources: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
        runtime: row.get(4)?,
        permission_mode: row.get(5)?,
        cadence: decode::<Cadence>(row, 6)?,
        review_cadence: decode::<Cadence>(row, 7)?,
        summary_cadence: decode::<Cadence>(row, 8)?,
        source_check_cadence: decode::<Cadence>(row, 9)?,
        quiet_hours,
        budget_usd: row.get(12)?,
        review_policy: decode::<ReviewPolicy>(row, 13)?,
        enabled: row.get::<_, i64>(14)? != 0,
        next_run_at: parse_optional_time(row.get(15)?)?,
        next_review_at: parse_optional_time(row.get(16)?)?,
        next_summary_at: parse_optional_time(row.get(17)?)?,
        next_source_check_at: parse_optional_time(row.get(18)?)?,
        created_at: parse_time(row.get(19)?)?,
        updated_at: parse_time(row.get(20)?)?,
    })
}

fn attention_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HumanAttention> {
    Ok(HumanAttention {
        id: row.get(0)?,
        run_id: row.get(1)?,
        goal_id: row.get(2)?,
        kind: decode::<AttentionKind>(row, 3)?,
        title: row.get(4)?,
        prompt: row.get(5)?,
        context: row.get(6)?,
        tool_name: row.get(7)?,
        tool_input: row.get(8)?,
        status: decode::<AttentionStatus>(row, 9)?,
        created_at: parse_time(row.get(10)?)?,
        resolved_at: parse_optional_time(row.get(11)?)?,
        response: row.get(12)?,
    })
}

fn goal_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GoalRun> {
    Ok(GoalRun {
        run_id: row.get(0)?,
        goal_id: row.get(1)?,
        work: decode::<WorkKind>(row, 2)?,
        created_at: parse_time(row.get(3)?)?,
    })
}

fn decode<T>(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let raw: String = row.get(index)?;
    raw.parse::<T>()
        .map_err(|err| conversion(index, Box::new(err)))
}

fn conversion(
    index: usize,
    err: Box<dyn std::error::Error + Send + Sync>,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, err)
}

fn parse_time(raw: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| conversion(0, Box::new(err)))
}

fn parse_optional_time(raw: Option<String>) -> rusqlite::Result<Option<DateTime<Utc>>> {
    match raw {
        Some(value) if !value.is_empty() => parse_time(value).map(Some),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use methodus_domain::{
        usage_month, AttentionKind, AttentionStatus, Cadence, GoalRun, HumanAttention,
        LearningGoal, QuietHours, ReviewPolicy, WorkKind,
    };

    use crate::Store;

    fn goal(id: &str) -> LearningGoal {
        let now = Utc::now();
        LearningGoal {
            id: id.into(),
            title: "Shutdown recovery".into(),
            prompt: "Understand the recovery path".into(),
            sources: vec!["docs/runbook".into(), "src/engine.rs".into()],
            runtime: "claude-code".into(),
            permission_mode: "plan".into(),
            cadence: Cadence::Weekly,
            review_cadence: Cadence::EveryHours(12),
            summary_cadence: Cadence::Monthly,
            source_check_cadence: Cadence::Manual,
            quiet_hours: Some(QuietHours::parse("22:00", "07:00").unwrap()),
            budget_usd: 20.0,
            review_policy: ReviewPolicy::MaintainerQuestions,
            enabled: true,
            next_run_at: Some(now + Duration::days(7)),
            next_review_at: None,
            next_summary_at: None,
            next_source_check_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn a_goal_round_trips_with_every_typed_field_intact() {
        let store = Store::open_memory().unwrap();
        let original = goal("goal_a");
        store.upsert_learning_goal(&original).unwrap();

        let loaded = store.learning_goal("goal_a").unwrap().unwrap();
        assert_eq!(loaded.sources, original.sources);
        assert_eq!(loaded.cadence, Cadence::Weekly);
        assert_eq!(loaded.review_cadence, Cadence::EveryHours(12));
        assert_eq!(loaded.source_check_cadence, Cadence::Manual);
        assert_eq!(loaded.quiet_hours, original.quiet_hours);
        assert_eq!(loaded.review_policy, ReviewPolicy::MaintainerQuestions);
        assert!(loaded.enabled);
        assert_eq!(
            loaded.next_run_at.map(|at| at.timestamp()),
            original.next_run_at.map(|at| at.timestamp())
        );
    }

    #[test]
    fn upsert_updates_in_place_rather_than_duplicating() {
        let store = Store::open_memory().unwrap();
        store.upsert_learning_goal(&goal("goal_a")).unwrap();

        let mut edited = goal("goal_a");
        edited.title = "Recovery, revised".into();
        edited.enabled = false;
        edited.quiet_hours = None;
        store.upsert_learning_goal(&edited).unwrap();

        let all = store.list_learning_goals().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Recovery, revised");
        assert!(!all[0].enabled);
        assert!(all[0].quiet_hours.is_none());
    }

    #[test]
    fn goals_list_enabled_first() {
        let store = Store::open_memory().unwrap();
        let mut disabled = goal("goal_a");
        disabled.title = "AAA disabled".into();
        disabled.enabled = false;
        store.upsert_learning_goal(&disabled).unwrap();

        let mut enabled = goal("goal_b");
        enabled.title = "ZZZ enabled".into();
        store.upsert_learning_goal(&enabled).unwrap();

        let all = store.list_learning_goals().unwrap();
        assert_eq!(all[0].id, "goal_b");
        assert_eq!(all[1].id, "goal_a");
    }

    fn attention(id: &str, run_id: &str) -> HumanAttention {
        HumanAttention {
            id: id.into(),
            run_id: run_id.into(),
            goal_id: Some("goal_a".into()),
            kind: AttentionKind::Question,
            title: "Which retry policy applies?".into(),
            prompt: "The runbook and the code disagree.".into(),
            context: Some("docs/runbook#retry".into()),
            tool_name: None,
            tool_input: None,
            status: AttentionStatus::Open,
            created_at: Utc::now(),
            resolved_at: None,
            response: None,
        }
    }

    #[test]
    fn resolving_attention_twice_only_succeeds_once() {
        let store = Store::open_memory().unwrap();
        store.insert_attention(&attention("att_a", "run_1")).unwrap();

        assert_eq!(store.list_open_attentions().unwrap().len(), 1);
        assert!(store.resolve_attention("att_a", "use the code", Utc::now()).unwrap());
        assert!(!store.resolve_attention("att_a", "no, the runbook", Utc::now()).unwrap());

        assert!(store.list_open_attentions().unwrap().is_empty());
        let stored = &store.list_attentions_for_run("run_1").unwrap()[0];
        assert_eq!(stored.status, AttentionStatus::Resolved);
        assert_eq!(stored.response.as_deref(), Some("use the code"));
        assert!(stored.resolved_at.is_some());
    }

    #[test]
    fn spend_accumulates_per_month_and_older_months_are_retained() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        let month = usage_month(now);

        assert_eq!(store.record_goal_spend("goal_a", &month, 4.0, now).unwrap(), 4.0);
        assert_eq!(store.record_goal_spend("goal_a", &month, 2.5, now).unwrap(), 6.5);
        store.record_goal_spend("goal_a", "2020-01", 99.0, now).unwrap();

        let current = store.goal_usage("goal_a", &month).unwrap().unwrap();
        assert_eq!(current.spent_usd, 6.5);
        assert!(current.exhausts(6.0));
        assert!(!current.exhausts(7.0));
        assert_eq!(store.goal_usage("goal_a", "2020-01").unwrap().unwrap().spent_usd, 99.0);
    }

    #[test]
    fn run_links_record_which_kind_of_turn_started_them() {
        let store = Store::open_memory().unwrap();
        let now = Utc::now();
        for (run_id, work) in [("run_1", WorkKind::Learn), ("run_2", WorkKind::Review)] {
            store
                .link_goal_run(&GoalRun {
                    run_id: run_id.into(),
                    goal_id: "goal_a".into(),
                    work,
                    created_at: now,
                })
                .unwrap();
        }

        assert_eq!(store.goal_run("run_2").unwrap().unwrap().work, WorkKind::Review);
        assert_eq!(store.list_goal_runs("goal_a", 10).unwrap().len(), 2);
        assert!(store.goal_run("run_missing").unwrap().is_none());
    }

    #[test]
    fn deleting_a_goal_reports_whether_it_existed() {
        let store = Store::open_memory().unwrap();
        store.upsert_learning_goal(&goal("goal_a")).unwrap();
        assert!(store.delete_learning_goal("goal_a").unwrap());
        assert!(!store.delete_learning_goal("goal_a").unwrap());
        assert!(store.learning_goal("goal_a").unwrap().is_none());
    }
}
