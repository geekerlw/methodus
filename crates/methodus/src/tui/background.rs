//! Unattended learning driven from the TUI's event loop.
//!
//! Scheduled turns run headless on the Tokio runtime while the maintainer keeps
//! using the terminal. Their results arrive as [`BackgroundEvent`]s, which the
//! UI drains once per frame. Nothing here decides *what* is due — that is
//! `methodus_core::learning::plan_tick`.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use methodus_core::learning::{BlockReason, BlockedGoal, TurnOutcome};
use methodus_core::Engine;
use tokio::runtime::Handle;

use crate::notify::NotifyUrgency;

/// How often the scheduler is consulted when nothing prompted it. A tick with
/// nothing due is two indexed SQLite reads, so this is not a cost worth tuning;
/// it only bounds how late a cadence or the end of quiet hours can be noticed.
/// Anything a person does that could make work due calls [`Background::wake`].
const TICK_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum BackgroundEvent {
    TurnStarted { goal_title: String, work: String },
    TurnFinished(Box<TurnOutcome>),
    TurnFailed { goal_id: String, goal_title: String, message: String },
    Blocked(Box<BlockedGoal>),
    SourcesChecked { newly_stale: usize },
    SchedulerFailed { message: String },
}

impl BackgroundEvent {
    /// The line shown in the TUI status bar.
    pub fn status_line(&self) -> String {
        match self {
            Self::TurnStarted { goal_title, work } => {
                format!("Scheduled {work}: {goal_title} running in the background")
            }
            Self::TurnFinished(outcome) => outcome.headline(),
            Self::TurnFailed { goal_title, message, .. } => {
                format!("{goal_title} failed: {message}")
            }
            Self::Blocked(blocked) => match blocked.reason {
                BlockReason::BudgetExhausted { spent_usd, budget_usd } => format!(
                    "{} skipped: ${spent_usd:.2} of ${budget_usd:.2} monthly budget spent",
                    blocked.goal_title
                ),
            },
            Self::SourcesChecked { newly_stale } => {
                format!("Source check: {newly_stale} node(s) went stale")
            }
            Self::SchedulerFailed { message } => format!("Scheduler failed: {message}"),
        }
    }

    /// The OS notification this event deserves, if any.
    ///
    /// Only things a person must act on are worth interrupting for; a turn that
    /// completed with nothing new is left to the status bar.
    pub fn notification(&self) -> Option<(String, String, NotifyUrgency)> {
        let (title, urgency) = match self {
            Self::TurnFinished(outcome) if outcome.needs_attention() => {
                ("Methodus needs you", NotifyUrgency::Critical)
            }
            Self::TurnFinished(outcome) if !outcome.candidate_ids.is_empty() => {
                ("Methodus review ready", NotifyUrgency::Normal)
            }
            Self::TurnFailed { .. } | Self::SchedulerFailed { .. } => {
                ("Methodus scheduler", NotifyUrgency::Critical)
            }
            Self::Blocked(_) => ("Methodus budget", NotifyUrgency::Normal),
            // Stale nodes are a standing fact, not an interruption: the next
            // review picks them up whether or not anyone reads the banner.
            Self::SourcesChecked { newly_stale } if *newly_stale > 0 => {
                ("Methodus sources", NotifyUrgency::Low)
            }
            _ => return None,
        };
        Some((title.to_string(), self.status_line(), urgency))
    }
}

pub struct Background {
    engine: Engine,
    runtime: Handle,
    sender: Sender<BackgroundEvent>,
    events: Receiver<BackgroundEvent>,
    /// Goals with a turn in flight. Kept on the UI thread; the scheduler must
    /// not hand the same Goal a second executor session.
    running: HashSet<String>,
    stale_nodes: HashSet<String>,
    next_tick: Instant,
}

impl Background {
    pub fn new(engine: Engine, runtime: Handle) -> Self {
        let (sender, events) = mpsc::channel();
        let stale_nodes = engine
            .list_graph_nodes(None)
            .map(|nodes| stale_ids(&nodes))
            .unwrap_or_default();
        Self {
            engine,
            runtime,
            sender,
            events,
            running: HashSet::new(),
            // Overdue work should fire as soon as the studio opens.
            next_tick: Instant::now(),
            stale_nodes,
        }
    }

    pub fn busy_count(&self) -> usize {
        self.running.len()
    }

    pub fn is_running(&self, goal_id: &str) -> bool {
        self.running.contains(goal_id)
    }

    /// Consult the scheduler on the next frame instead of waiting out the poll
    /// interval. Called after anything a person did that can make work due, so
    /// "run now" means now rather than "within a minute".
    pub fn wake(&mut self) {
        self.next_tick = Instant::now();
    }

    /// Consult the scheduler and launch whatever is due.
    ///
    /// `foreground_goal` is the Goal the maintainer is working on interactively;
    /// it is reported as occupied so a background turn cannot resume the same
    /// executor session underneath them.
    pub fn tick(&mut self, foreground_goal: Option<&str>) {
        if Instant::now() < self.next_tick {
            return;
        }
        self.next_tick = Instant::now() + TICK_INTERVAL;

        let mut occupied = self.running.clone();
        occupied.extend(foreground_goal.map(str::to_string));
        let plan = match self.engine.plan_learning_tick(occupied) {
            Ok(plan) => plan,
            Err(error) => {
                self.emit(BackgroundEvent::SchedulerFailed { message: error.to_string() });
                return;
            }
        };
        if plan.is_empty() {
            return;
        }

        for blocked in plan.blocked {
            self.emit(BackgroundEvent::Blocked(Box::new(blocked)));
        }
        if plan.source_check_due {
            self.check_sources();
        }
        for turn in plan.turns {
            self.running.insert(turn.goal.id.clone());
            self.emit(BackgroundEvent::TurnStarted {
                goal_title: turn.goal.title.clone(),
                work: turn.work.to_string(),
            });
            let engine = self.engine.clone();
            let sender = self.sender.clone();
            let goal_id = turn.goal.id.clone();
            let goal_title = turn.goal.title.clone();
            self.runtime.spawn(async move {
                let event = match engine.run_scheduled_turn(&turn).await {
                    Ok(outcome) => BackgroundEvent::TurnFinished(Box::new(outcome)),
                    Err(error) => BackgroundEvent::TurnFailed {
                        goal_id,
                        goal_title,
                        message: error.to_string(),
                    },
                };
                let _ = sender.send(event);
            });
        }
    }

    /// Collect finished work. Called once per frame; never blocks.
    pub fn drain(&mut self) -> Vec<BackgroundEvent> {
        let mut collected = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match &event {
                BackgroundEvent::TurnFinished(outcome) => {
                    self.running.remove(&outcome.goal_id);
                }
                BackgroundEvent::TurnFailed { goal_id, .. } => {
                    self.running.remove(goal_id);
                }
                _ => {}
            }
            collected.push(event);
        }
        collected
    }

    /// Re-index the Markdown graph and report nodes that just went stale.
    /// Only the transition is reported; a node that was already stale yesterday
    /// is not news.
    fn check_sources(&mut self) {
        if self.engine.sync_graph().is_err() {
            return;
        }
        let Ok(nodes) = self.engine.list_graph_nodes(None) else {
            return;
        };
        let stale = stale_ids(&nodes);
        let newly_stale = stale.difference(&self.stale_nodes).count();
        self.stale_nodes = stale;
        if newly_stale > 0 {
            self.emit(BackgroundEvent::SourcesChecked { newly_stale });
        }
    }

    fn emit(&self, event: BackgroundEvent) {
        let _ = self.sender.send(event);
    }
}

fn stale_ids(nodes: &[methodus_domain::GraphNode]) -> HashSet<String> {
    nodes
        .iter()
        .filter(|node| node.status.as_deref() == Some("stale"))
        .map(|node| node.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Utc;
    use methodus_domain::{AttentionKind, AttentionStatus, HumanAttention, WorkKind};

    fn outcome() -> TurnOutcome {
        TurnOutcome {
            run_id: "learn_1".into(),
            goal_id: "goal_a".into(),
            goal_title: "Shutdown recovery".into(),
            work: WorkKind::Learn,
            candidate_ids: Vec::new(),
            attention: None,
            failure: None,
            cost_usd: 0.5,
            spent_usd: 4.5,
            budget_usd: 20.0,
        }
    }

    fn attention() -> HumanAttention {
        HumanAttention {
            id: "att_1".into(),
            run_id: "learn_1".into(),
            goal_id: Some("goal_a".into()),
            kind: AttentionKind::Question,
            title: "Which source wins?".into(),
            prompt: "Two sources disagree.".into(),
            context: None,
            tool_name: None,
            tool_input: None,
            status: AttentionStatus::Open,
            created_at: Utc::now(),
            resolved_at: None,
            response: None,
        }
    }

    #[test]
    fn a_quiet_completion_does_not_interrupt_anyone() {
        let event = BackgroundEvent::TurnFinished(Box::new(outcome()));
        assert!(event.notification().is_none());
        assert!(event.status_line().contains("no new candidates"));
    }

    #[test]
    fn review_ready_notifies_without_a_sound() {
        let mut done = outcome();
        done.candidate_ids = vec!["knowledge/candidate-1".into()];
        let (title, body, urgency) = BackgroundEvent::TurnFinished(Box::new(done))
            .notification()
            .expect("a finished turn with candidates should notify");
        assert_eq!(title, "Methodus review ready");
        assert_eq!(urgency, NotifyUrgency::Normal);
        assert!(body.contains("1 candidate"));
    }

    #[test]
    fn work_that_blocks_on_a_person_is_critical() {
        let mut blocked = outcome();
        blocked.attention = Some(attention());
        let (title, _, urgency) = BackgroundEvent::TurnFinished(Box::new(blocked))
            .notification()
            .expect("a hand-off should notify");
        assert_eq!(title, "Methodus needs you");
        assert_eq!(urgency, NotifyUrgency::Critical);
    }

    #[test]
    fn a_failed_turn_is_critical_and_names_the_goal() {
        let event = BackgroundEvent::TurnFailed {
            goal_id: "goal_a".into(),
            goal_title: "Shutdown recovery".into(),
            message: "runtime not found".into(),
        };
        let (_, body, urgency) = event.notification().expect("a failure should notify");
        assert_eq!(urgency, NotifyUrgency::Critical);
        assert!(body.contains("Shutdown recovery"));
        assert!(body.contains("runtime not found"));
    }

    #[test]
    fn an_exhausted_budget_reports_both_numbers() {
        let event = BackgroundEvent::Blocked(Box::new(BlockedGoal {
            goal_id: "goal_a".into(),
            goal_title: "Shutdown recovery".into(),
            work: WorkKind::Learn,
            reason: BlockReason::BudgetExhausted { spent_usd: 21.0, budget_usd: 20.0 },
        }));
        let line = event.status_line();
        assert!(line.contains("$21.00"), "{line}");
        assert!(line.contains("$20.00"), "{line}");
        assert_eq!(event.notification().unwrap().2, NotifyUrgency::Normal);
    }

    #[test]
    fn unchanged_sources_stay_silent() {
        assert!(BackgroundEvent::SourcesChecked { newly_stale: 0 }.notification().is_none());
        assert!(BackgroundEvent::SourcesChecked { newly_stale: 3 }.notification().is_some());
    }

    #[test]
    fn a_started_turn_is_status_only() {
        let event = BackgroundEvent::TurnStarted {
            goal_title: "Shutdown recovery".into(),
            work: "learn".into(),
        };
        assert!(event.notification().is_none());
        assert!(event.status_line().contains("Shutdown recovery"));
    }
}
