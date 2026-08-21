//! Continuous learning: goals, schedules, human attention, and budget.
//!
//! Everything here is surface-independent. A TUI, a CLI, or any future client
//! renders the same records and drives the same tick; none of them may hold a
//! private copy of this policy.

pub mod attention;
pub mod goal;
pub mod schedule;
pub mod turn;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use methodus_domain::{usage_month, AttentionStatus, HumanAttention};
use methodus_store::Store;

use crate::error::CoreError;

pub use attention::{envelope_title, parse_envelope, AttentionEnvelope};
pub use goal::{goal_prompt_for, parse_form, render_form, GoalForm};
pub use schedule::{
    plan_tick, BlockReason, BlockedGoal, ScheduledTurn, TickInput, TickPlan,
};
pub use turn::{classify, TurnDisposition, TurnOutcome, TurnTranscript};

/// Charge a completed turn against its Goal's monthly budget and return the new
/// total. Non-positive and non-finite costs are ignored so a runtime that
/// reports no usage cannot corrupt the ledger.
pub fn record_spend(
    store: &Store,
    goal_id: &str,
    cost_usd: f64,
    now: DateTime<Utc>,
) -> Result<f64, CoreError> {
    if !cost_usd.is_finite() || cost_usd <= 0.0 {
        return goal_spend(store, goal_id, now);
    }
    Ok(store.record_goal_spend(goal_id, &usage_month(now), cost_usd, now)?)
}

/// This month's spend for one Goal.
pub fn goal_spend(store: &Store, goal_id: &str, now: DateTime<Utc>) -> Result<f64, CoreError> {
    Ok(store
        .goal_usage(goal_id, &usage_month(now))?
        .map(|usage| usage.spent_usd)
        .unwrap_or_default())
}

/// Record that a run stopped and needs a person.
pub fn open_attention(
    store: &Store,
    run_id: &str,
    goal_id: Option<String>,
    envelope: &AttentionEnvelope,
    now: DateTime<Utc>,
) -> Result<HumanAttention, CoreError> {
    let attention = HumanAttention {
        id: format!("att_{}", Uuid::new_v4().simple()),
        run_id: run_id.to_string(),
        goal_id,
        kind: envelope.kind,
        title: envelope_title(envelope),
        prompt: envelope.question.clone(),
        context: envelope.context.clone(),
        tool_name: envelope.tool_name.clone(),
        tool_input: envelope.tool_input.clone(),
        status: AttentionStatus::Open,
        created_at: now,
        resolved_at: None,
        response: None,
    };
    store.insert_attention(&attention)?;
    Ok(attention)
}

/// The most recent unanswered hand-off for a run, if any.
pub fn attention_for_run(store: &Store, run_id: &str) -> Result<Option<HumanAttention>, CoreError> {
    Ok(store
        .list_attentions_for_run(run_id)?
        .into_iter()
        .rfind(HumanAttention::is_open))
}

#[cfg(test)]
mod tests {
    use super::*;

    use methodus_domain::AttentionKind;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
    }

    fn envelope(question: &str) -> AttentionEnvelope {
        AttentionEnvelope {
            kind: AttentionKind::Question,
            question: question.to_string(),
            context: None,
            tool_name: None,
            tool_input: None,
        }
    }

    #[test]
    fn spend_accumulates_and_invalid_costs_are_ignored() {
        let store = Store::open_memory().unwrap();
        let now = at("2026-08-21T00:00:00Z");

        assert_eq!(record_spend(&store, "goal_a", 3.0, now).unwrap(), 3.0);
        assert_eq!(record_spend(&store, "goal_a", f64::NAN, now).unwrap(), 3.0);
        assert_eq!(record_spend(&store, "goal_a", -5.0, now).unwrap(), 3.0);
        assert_eq!(record_spend(&store, "goal_a", 1.5, now).unwrap(), 4.5);
        assert_eq!(goal_spend(&store, "goal_a", now).unwrap(), 4.5);
    }

    #[test]
    fn spend_is_scoped_to_the_calendar_month() {
        let store = Store::open_memory().unwrap();
        record_spend(&store, "goal_a", 9.0, at("2026-08-31T23:00:00Z")).unwrap();
        assert_eq!(goal_spend(&store, "goal_a", at("2026-09-01T01:00:00Z")).unwrap(), 0.0);
    }

    #[test]
    fn the_latest_open_hand_off_wins_and_resolving_clears_it() {
        let store = Store::open_memory().unwrap();
        open_attention(&store, "run_1", None, &envelope("first"), at("2026-08-21T00:00:00Z")).unwrap();
        let second = open_attention(
            &store,
            "run_1",
            Some("goal_a".into()),
            &envelope("second"),
            at("2026-08-21T01:00:00Z"),
        )
        .unwrap();

        let latest = attention_for_run(&store, "run_1").unwrap().unwrap();
        assert_eq!(latest.id, second.id);
        assert_eq!(latest.title, "second");

        store.resolve_attention(&second.id, "answered", at("2026-08-21T02:00:00Z")).unwrap();
        assert_eq!(attention_for_run(&store, "run_1").unwrap().unwrap().title, "first");
    }

    #[test]
    fn a_run_with_no_hand_off_reports_none() {
        let store = Store::open_memory().unwrap();
        assert!(attention_for_run(&store, "run_missing").unwrap().is_none());
    }
}
