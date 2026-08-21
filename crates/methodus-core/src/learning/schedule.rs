//! Deciding which learning turns are due right now.
//!
//! Planning is separated from execution: this module reads the Goal schedules,
//! advances the ones it selects, and returns a plan. Launching runtimes is the
//! caller's job, because only the surface owning the sessions can do it.

use std::collections::HashSet;

use chrono::{DateTime, NaiveTime, Utc};

use methodus_domain::{usage_month, LearningGoal, WorkKind};
use methodus_store::Store;

use crate::error::CoreError;
use crate::learning::goal::goal_prompt_for;

/// Everything a tick needs that the store cannot supply on its own.
#[derive(Debug, Clone)]
pub struct TickInput {
    /// Goals that currently hold a runtime session. Live sessions belong to the
    /// surface, so this set is passed in rather than queried.
    pub occupied_goal_ids: HashSet<String>,
    pub now: DateTime<Utc>,
    /// Local wall-clock time, used only to evaluate quiet hours.
    pub local_time: NaiveTime,
}

/// A turn the caller should launch.
#[derive(Debug, Clone)]
pub struct ScheduledTurn {
    pub goal: LearningGoal,
    pub work: WorkKind,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockReason {
    BudgetExhausted { spent_usd: f64, budget_usd: f64 },
}

/// A turn that came due but must not run.
#[derive(Debug, Clone)]
pub struct BlockedGoal {
    pub goal_id: String,
    pub goal_title: String,
    pub work: WorkKind,
    pub reason: BlockReason,
}

#[derive(Debug, Clone, Default)]
pub struct TickPlan {
    pub turns: Vec<ScheduledTurn>,
    pub blocked: Vec<BlockedGoal>,
    pub source_check_due: bool,
}

impl TickPlan {
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() && self.blocked.is_empty() && !self.source_check_due
    }
}

/// Select the work due at `input.now` and persist the advanced schedules.
///
/// Advancing is part of planning on purpose: without it the same turn would be
/// selected again on the next tick before the caller has finished launching it.
pub fn plan_tick(store: &Store, input: &TickInput) -> Result<TickPlan, CoreError> {
    let awaiting_human: HashSet<String> = store
        .list_open_attentions()?
        .into_iter()
        .filter_map(|attention| attention.goal_id)
        .collect();

    let mut plan = TickPlan::default();
    for mut goal in store
        .list_learning_goals()?
        .into_iter()
        .filter(|goal| goal.enabled)
    {
        // Quiet hours defer without advancing, so deferred work runs as soon as
        // the window closes instead of slipping a whole cadence.
        if goal.is_quiet_at(input.local_time) {
            continue;
        }

        let mut advanced = false;
        let free = !input.occupied_goal_ids.contains(&goal.id)
            && !awaiting_human.contains(&goal.id);
        if free {
            // A Goal owns one executor session at a time, so at most one runtime
            // turn is selected per tick; the others stay due for a later tick.
            let due = WorkKind::RUNTIME_TURNS
                .iter()
                .copied()
                .find(|work| goal.is_due(*work, input.now));
            if let Some(work) = due {
                goal.advance(work, input.now);
                advanced = true;
                match budget_overrun(store, &goal, input.now)? {
                    Some(spent_usd) => plan.blocked.push(BlockedGoal {
                        goal_id: goal.id.clone(),
                        goal_title: goal.title.clone(),
                        work,
                        reason: BlockReason::BudgetExhausted {
                            spent_usd,
                            budget_usd: goal.budget_usd,
                        },
                    }),
                    None => plan.turns.push(ScheduledTurn {
                        prompt: goal_prompt_for(&goal, work),
                        goal: goal.clone(),
                        work,
                    }),
                }
            }
        }

        // A source check reads the index and never occupies the runtime, so it
        // proceeds even while the Goal is busy or waiting on a person.
        if goal.is_due(WorkKind::SourceCheck, input.now) {
            goal.advance(WorkKind::SourceCheck, input.now);
            plan.source_check_due = true;
            advanced = true;
        }

        if advanced {
            goal.updated_at = input.now;
            store.upsert_learning_goal(&goal)?;
        }
    }
    Ok(plan)
}

fn budget_overrun(
    store: &Store,
    goal: &LearningGoal,
    now: DateTime<Utc>,
) -> Result<Option<f64>, CoreError> {
    Ok(store
        .goal_usage(&goal.id, &usage_month(now))?
        .filter(|usage| usage.exhausts(goal.budget_usd))
        .map(|usage| usage.spent_usd))
}

#[cfg(test)]
mod tests {
    use super::*;

    use methodus_domain::{
        AttentionKind, AttentionStatus, Cadence, HumanAttention, QuietHours,
    };

    use crate::learning::goal::GoalForm;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
    }

    fn noon() -> NaiveTime {
        NaiveTime::from_hms_opt(12, 0, 0).unwrap()
    }

    fn input(now: &str) -> TickInput {
        TickInput {
            occupied_goal_ids: HashSet::new(),
            now: at(now),
            local_time: noon(),
        }
    }

    fn store_with_goal(form: GoalForm, created: &str) -> (Store, LearningGoal) {
        let store = Store::open_memory().unwrap();
        let goal = form.into_new_goal(at(created)).unwrap();
        store.upsert_learning_goal(&goal).unwrap();
        (store, goal)
    }

    fn daily_learning() -> GoalForm {
        GoalForm {
            cadence: Cadence::Daily,
            review_cadence: Cadence::Manual,
            summary_cadence: Cadence::Manual,
            source_check_cadence: Cadence::Manual,
            ..GoalForm::default()
        }
    }

    #[test]
    fn nothing_is_due_before_the_first_cadence_elapses() {
        let (store, _) = store_with_goal(daily_learning(), "2026-08-21T00:00:00Z");
        let plan = plan_tick(&store, &input("2026-08-21T12:00:00Z")).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn a_due_learning_turn_is_selected_and_its_schedule_advances() {
        let (store, goal) = store_with_goal(daily_learning(), "2026-08-21T00:00:00Z");
        let now = "2026-08-22T12:00:00Z";
        let plan = plan_tick(&store, &input(now)).unwrap();

        assert_eq!(plan.turns.len(), 1);
        assert_eq!(plan.turns[0].work, WorkKind::Learn);
        assert!(plan.turns[0].prompt.contains("Scheduled learning"));

        let stored = store.learning_goal(&goal.id).unwrap().unwrap();
        assert_eq!(stored.next_run_at, Some(at("2026-08-23T12:00:00Z")));
        // The same tick repeated must not launch the turn twice.
        assert!(plan_tick(&store, &input(now)).unwrap().is_empty());
    }

    #[test]
    fn only_one_runtime_turn_is_selected_when_several_are_due() {
        let form = GoalForm {
            cadence: Cadence::Daily,
            review_cadence: Cadence::Daily,
            summary_cadence: Cadence::Daily,
            source_check_cadence: Cadence::Manual,
            ..GoalForm::default()
        };
        let (store, goal) = store_with_goal(form, "2026-08-21T00:00:00Z");
        let plan = plan_tick(&store, &input("2026-08-25T00:00:00Z")).unwrap();

        assert_eq!(plan.turns.len(), 1);
        assert_eq!(plan.turns[0].work, WorkKind::Learn);

        // Review and summary stay due so a later tick can pick them up.
        let stored = store.learning_goal(&goal.id).unwrap().unwrap();
        assert!(stored.is_due(WorkKind::Review, at("2026-08-25T00:00:00Z")));
        assert!(stored.is_due(WorkKind::Summary, at("2026-08-25T00:00:00Z")));
    }

    #[test]
    fn a_goal_holding_a_session_is_skipped_without_losing_its_turn() {
        let (store, goal) = store_with_goal(daily_learning(), "2026-08-21T00:00:00Z");
        let mut busy = input("2026-08-22T12:00:00Z");
        busy.occupied_goal_ids.insert(goal.id.clone());

        assert!(plan_tick(&store, &busy).unwrap().is_empty());
        // Once the session ends the same turn is still waiting.
        assert_eq!(
            plan_tick(&store, &input("2026-08-22T12:00:00Z")).unwrap().turns.len(),
            1
        );
    }

    #[test]
    fn a_goal_awaiting_a_human_answer_is_not_given_more_work() {
        let (store, goal) = store_with_goal(daily_learning(), "2026-08-21T00:00:00Z");
        let attention = HumanAttention {
            id: "att_1".into(),
            run_id: "run_1".into(),
            goal_id: Some(goal.id.clone()),
            kind: AttentionKind::Question,
            title: "Which policy?".into(),
            prompt: "Two sources disagree.".into(),
            context: None,
            tool_name: None,
            tool_input: None,
            status: AttentionStatus::Open,
            created_at: at("2026-08-21T06:00:00Z"),
            resolved_at: None,
            response: None,
        };
        store.insert_attention(&attention).unwrap();
        assert!(plan_tick(&store, &input("2026-08-22T12:00:00Z")).unwrap().is_empty());

        store.resolve_attention("att_1", "use the code", at("2026-08-22T11:00:00Z")).unwrap();
        assert_eq!(
            plan_tick(&store, &input("2026-08-22T12:00:00Z")).unwrap().turns.len(),
            1
        );
    }

    #[test]
    fn quiet_hours_defer_without_consuming_the_schedule() {
        let form = GoalForm {
            quiet_hours: Some(QuietHours::parse("22:00", "07:00").unwrap()),
            ..daily_learning()
        };
        let (store, goal) = store_with_goal(form, "2026-08-21T00:00:00Z");

        let mut quiet = input("2026-08-22T12:00:00Z");
        quiet.local_time = NaiveTime::from_hms_opt(23, 30, 0).unwrap();
        assert!(plan_tick(&store, &quiet).unwrap().is_empty());
        assert_eq!(
            store.learning_goal(&goal.id).unwrap().unwrap().next_run_at,
            Some(at("2026-08-22T00:00:00Z"))
        );

        assert_eq!(
            plan_tick(&store, &input("2026-08-22T12:00:00Z")).unwrap().turns.len(),
            1
        );
    }

    #[test]
    fn an_exhausted_budget_blocks_the_turn_and_reports_the_overrun() {
        let (store, goal) = store_with_goal(daily_learning(), "2026-08-21T00:00:00Z");
        let now = at("2026-08-22T12:00:00Z");
        store
            .record_goal_spend(&goal.id, &usage_month(now), 25.0, now)
            .unwrap();

        let plan = plan_tick(&store, &input("2026-08-22T12:00:00Z")).unwrap();
        assert!(plan.turns.is_empty());
        assert_eq!(plan.blocked.len(), 1);
        assert_eq!(
            plan.blocked[0].reason,
            BlockReason::BudgetExhausted { spent_usd: 25.0, budget_usd: 20.0 }
        );
    }

    #[test]
    fn a_source_check_runs_even_while_the_goal_is_busy() {
        let form = GoalForm {
            source_check_cadence: Cadence::Daily,
            ..daily_learning()
        };
        let (store, goal) = store_with_goal(form, "2026-08-21T00:00:00Z");
        let mut busy = input("2026-08-22T12:00:00Z");
        busy.occupied_goal_ids.insert(goal.id.clone());

        let plan = plan_tick(&store, &busy).unwrap();
        assert!(plan.turns.is_empty());
        assert!(plan.source_check_due);
        assert!(!plan_tick(&store, &busy).unwrap().source_check_due);
    }

    #[test]
    fn a_disabled_goal_is_never_planned() {
        let form = GoalForm { enabled: false, ..daily_learning() };
        let (store, _) = store_with_goal(form, "2026-08-21T00:00:00Z");
        assert!(plan_tick(&store, &input("2027-01-01T00:00:00Z")).unwrap().is_empty());
    }
}
