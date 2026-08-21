//! Authoring and validating learning goals.
//!
//! Goals are edited as YAML in `$EDITOR` rather than through a form widget, so
//! the editable surface is defined once here and every surface reuses it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use methodus_domain::{Cadence, LearningGoal, QuietHours, ReviewPolicy, WorkKind};

use crate::error::CoreError;

const RUNTIMES: [&str; 3] = ["claude-code", "codex", "cursor"];
const PERMISSION_MODES: [&str; 3] = ["plan", "cautious", "acceptEdits"];

/// Comment block prepended to the editable form. `serde_yaml` cannot emit
/// comments, so the legend lives here instead of beside each field.
const FORM_HEADER: &str = "\
# Methodus learning goal.
#
# Save and exit to apply. Quit without saving to cancel.
#
#   cadence, review_cadence, summary_cadence, source_check_cadence
#                     manual | daily | weekly | monthly | every:<hours>
#   runtime           claude-code | codex | cursor
#   permission_mode   plan | cautious | acceptEdits
#   review_policy     human_required | maintainer_questions
#   quiet_hours       omit for none, or set both start and end as \"HH:MM\"
#   sources           @-mention paths Methodus authorizes as evidence
#
";

/// The fields a maintainer may edit. System-owned values (id, timestamps, the
/// computed schedule) are deliberately absent so an edit cannot corrupt them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalForm {
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub sources: Vec<String>,
    pub runtime: String,
    pub permission_mode: String,
    pub cadence: Cadence,
    pub review_cadence: Cadence,
    pub summary_cadence: Cadence,
    pub source_check_cadence: Cadence,
    #[serde(default)]
    pub quiet_hours: Option<QuietHours>,
    pub budget_usd: f64,
    pub review_policy: ReviewPolicy,
    pub enabled: bool,
}

impl Default for GoalForm {
    fn default() -> Self {
        Self {
            title: "Untitled goal".into(),
            prompt: "Describe what Methodus should investigate, and what evidence would settle it.".into(),
            sources: Vec::new(),
            runtime: "claude-code".into(),
            permission_mode: "plan".into(),
            cadence: Cadence::Weekly,
            review_cadence: Cadence::Weekly,
            summary_cadence: Cadence::Monthly,
            source_check_cadence: Cadence::Daily,
            quiet_hours: None,
            budget_usd: 20.0,
            review_policy: ReviewPolicy::HumanRequired,
            enabled: true,
        }
    }
}

impl GoalForm {
    /// Build a form from one stretch of natural language, the way Learn accepts
    /// a goal.
    ///
    /// Stating the objective is enough to create a Goal; every policy field
    /// takes its default and the YAML form refines them afterwards. Asking
    /// someone to choose four cadences and a budget before they have said what
    /// they want is the wrong order.
    pub fn from_objective(objective: &str) -> Self {
        Self {
            title: title_from(objective),
            prompt: objective.trim().to_string(),
            ..Self::default()
        }
    }

    pub fn from_goal(goal: &LearningGoal) -> Self {
        Self {
            title: goal.title.clone(),
            prompt: goal.prompt.clone(),
            sources: goal.sources.clone(),
            runtime: goal.runtime.clone(),
            permission_mode: goal.permission_mode.clone(),
            cadence: goal.cadence,
            review_cadence: goal.review_cadence,
            summary_cadence: goal.summary_cadence,
            source_check_cadence: goal.source_check_cadence,
            quiet_hours: goal.quiet_hours,
            budget_usd: goal.budget_usd,
            review_policy: goal.review_policy,
            enabled: goal.enabled,
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if self.title.trim().is_empty() {
            return Err(invalid("goal title cannot be empty"));
        }
        if self.prompt.trim().is_empty() {
            return Err(invalid("goal prompt cannot be empty"));
        }
        if !RUNTIMES.contains(&self.runtime.as_str()) {
            return Err(invalid("runtime must be claude-code, codex, or cursor"));
        }
        if !PERMISSION_MODES.contains(&self.permission_mode.as_str()) {
            return Err(invalid(
                "permission_mode must be plan, cautious, or acceptEdits",
            ));
        }
        if !self.budget_usd.is_finite() || self.budget_usd <= 0.0 {
            return Err(invalid("budget_usd must be greater than zero"));
        }
        Ok(())
    }

    /// Build a brand-new Goal with its schedule primed from `now`.
    pub fn into_new_goal(self, now: DateTime<Utc>) -> Result<LearningGoal, CoreError> {
        self.validate()?;
        let mut goal = LearningGoal {
            id: format!("goal_{}", Uuid::new_v4().simple()),
            title: self.title.trim().to_string(),
            prompt: self.prompt.trim().to_string(),
            sources: self.sources,
            runtime: self.runtime,
            permission_mode: self.permission_mode,
            cadence: self.cadence,
            review_cadence: self.review_cadence,
            summary_cadence: self.summary_cadence,
            source_check_cadence: self.source_check_cadence,
            quiet_hours: self.quiet_hours,
            budget_usd: self.budget_usd,
            review_policy: self.review_policy,
            enabled: self.enabled,
            next_run_at: None,
            next_review_at: None,
            next_summary_at: None,
            next_source_check_at: None,
            created_at: now,
            updated_at: now,
        };
        goal.reschedule_all(now);
        Ok(goal)
    }

    /// Apply an edit to an existing Goal, preserving its identity and history.
    ///
    /// Pending due timestamps survive unless their cadence changed, so editing
    /// an unrelated field does not silently postpone work that was already due.
    pub fn apply_to(self, goal: &mut LearningGoal, now: DateTime<Utc>) -> Result<(), CoreError> {
        self.validate()?;
        let previous: Vec<(WorkKind, Cadence)> = WorkKind::ALL
            .iter()
            .map(|work| (*work, goal.cadence_for(*work)))
            .collect();
        let was_enabled = goal.enabled;

        goal.title = self.title.trim().to_string();
        goal.prompt = self.prompt.trim().to_string();
        goal.sources = self.sources;
        goal.runtime = self.runtime;
        goal.permission_mode = self.permission_mode;
        goal.cadence = self.cadence;
        goal.review_cadence = self.review_cadence;
        goal.summary_cadence = self.summary_cadence;
        goal.source_check_cadence = self.source_check_cadence;
        goal.quiet_hours = self.quiet_hours;
        goal.budget_usd = self.budget_usd;
        goal.review_policy = self.review_policy;
        goal.enabled = self.enabled;
        goal.updated_at = now;

        if !goal.enabled {
            for work in WorkKind::ALL {
                goal.set_next_at(work, None);
            }
            return Ok(());
        }
        for (work, before) in previous {
            let cadence_changed = before != goal.cadence_for(work);
            if !was_enabled || cadence_changed || goal.next_at(work).is_none() {
                goal.set_next_at(work, goal.cadence_for(work).next_after(now));
            }
        }
        Ok(())
    }
}

/// The label for an objective: its first sentence, clipped only when it is long
/// enough to be a paragraph. Narrowing a title to a column is the renderer's
/// job, so the limit here is generous. Clipping counts characters rather than
/// bytes so CJK objectives, which carry no spaces to break on, survive it.
fn title_from(objective: &str) -> String {
    const LIMIT: usize = 72;
    let sentence = objective
        .trim()
        .split(['.', '!', '?', '\n', '。', '！', '？'])
        .find(|part| !part.trim().is_empty())
        .unwrap_or("")
        .trim();
    if sentence.is_empty() {
        return "Untitled goal".into();
    }
    if sentence.chars().count() <= LIMIT {
        return sentence.to_string();
    }
    let head: String = sentence.chars().take(LIMIT).collect();
    let clipped = head.rsplit_once(' ').map(|(before, _)| before).unwrap_or(&head);
    format!("{}…", clipped.trim_end())
}

/// Render the editable YAML document handed to `$EDITOR`.
pub fn render_form(form: &GoalForm) -> Result<String, CoreError> {
    let body = serde_yaml::to_string(form)
        .map_err(|error| invalid(&format!("could not render goal form: {error}")))?;
    Ok(format!("{FORM_HEADER}{body}"))
}

/// Parse the document returned by `$EDITOR`.
pub fn parse_form(text: &str) -> Result<GoalForm, CoreError> {
    let form: GoalForm = serde_yaml::from_str(text)
        .map_err(|error| invalid(&format!("could not parse goal form: {error}")))?;
    form.validate()?;
    Ok(form)
}

/// The brief handed to the runtime for one scheduled turn.
///
/// The cadence and review policy are restated in the prompt so the runtime knows
/// the rhythm it is part of, but every enforcement decision stays in Rust.
pub fn goal_prompt_for(goal: &LearningGoal, work: WorkKind) -> String {
    let sources = if goal.sources.is_empty() {
        "none specified".to_string()
    } else {
        goal.sources
            .iter()
            .map(|source| format!("- @{source}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let instructions = match work {
        WorkKind::Review => "Scheduled review: inspect the currently published Methodus knowledge related to this goal, re-check its evidence and source freshness, and propose only narrowly scoped CandidateSet revisions for anything stale, contradicted, or incomplete.",
        WorkKind::Summary => "Scheduled synthesis: summarize what is now known, what changed, unresolved questions, and the next useful learning step. Create CandidateSet entries only for durable knowledge or methods that deserve human Review.",
        WorkKind::Learn | WorkKind::SourceCheck => "Scheduled learning: investigate the goal, challenge assumptions, compare evidence, and return a CandidateSet only when the evidence is sufficient.",
    };
    format!(
        "{}\n\n{}\n\nAuthorized evidence sources (inspect these explicitly):\n{}\n\n\
         Execution policy: monthly budget ${:.2}; review cadence {}; summary cadence {}; \
         source checks {}; review policy {}; never publish canonical knowledge without a human decision.",
        goal.prompt,
        instructions,
        sources,
        goal.budget_usd,
        goal.review_cadence,
        goal.summary_cadence,
        goal.source_check_cadence,
        goal.review_policy,
    )
}

fn invalid(message: &str) -> CoreError {
    CoreError::Other(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn the_default_form_round_trips_through_yaml() {
        let rendered = render_form(&GoalForm::default()).unwrap();
        assert!(rendered.starts_with("# Methodus learning goal."));
        assert_eq!(parse_form(&rendered).unwrap(), GoalForm::default());
    }

    #[test]
    fn an_edited_form_round_trips_including_quiet_hours() {
        let form = GoalForm {
            title: "Shutdown recovery".into(),
            sources: vec!["docs/runbook.md".into()],
            cadence: Cadence::EveryHours(6),
            quiet_hours: Some(QuietHours::parse("22:00", "07:00").unwrap()),
            review_policy: ReviewPolicy::MaintainerQuestions,
            ..GoalForm::default()
        };

        let parsed = parse_form(&render_form(&form).unwrap()).unwrap();
        assert_eq!(parsed, form);
    }

    #[test]
    fn a_malformed_cadence_is_rejected_at_parse_time() {
        let mut text = render_form(&GoalForm::default()).unwrap();
        text = text.replace("cadence: weekly", "cadence: fortnightly");
        let error = parse_form(&text).unwrap_err().to_string();
        assert!(error.contains("could not parse goal form"), "{error}");
    }

    #[test]
    fn validation_rejects_empty_prompts_and_unknown_runtimes() {
        let blank = GoalForm { prompt: "   ".into(), ..GoalForm::default() };
        assert!(blank.validate().unwrap_err().to_string().contains("prompt"));

        let unknown = GoalForm { runtime: "gemini".into(), ..GoalForm::default() };
        assert!(unknown.validate().unwrap_err().to_string().contains("runtime"));

        let free = GoalForm { budget_usd: 0.0, ..GoalForm::default() };
        assert!(free.validate().unwrap_err().to_string().contains("budget"));
    }

    #[test]
    fn stating_an_objective_is_enough_to_build_a_goal() {
        let form = GoalForm::from_objective(
            "  Understand how we diagnose abnormal device shutdown. Start with the runbook.  ",
        );
        assert_eq!(form.title, "Understand how we diagnose abnormal device shutdown");
        assert!(form.prompt.starts_with("Understand how"));
        assert!(form.prompt.ends_with("the runbook."));
        assert_eq!(form.cadence, GoalForm::default().cadence);
        form.validate().unwrap();
    }

    #[test]
    fn a_long_objective_is_clipped_into_a_readable_title() {
        let sentence = "Maintain current evidence-backed knowledge of cancellation, \
                        scheduling, production failure modes, and applicability";
        let title = GoalForm::from_objective(sentence).title;
        assert!(title.ends_with('…'), "{title}");
        assert!(title.chars().count() <= 73, "{title}");
        assert!(sentence.starts_with(title.trim_end_matches('…')), "{title}");
        // Clipping happens on a word boundary rather than mid-word.
        assert!(!title.trim_end_matches('…').ends_with(char::is_whitespace));

        // CJK carries no spaces to break on, so clipping must count characters
        // and must not panic on a byte index inside one.
        let cjk = GoalForm::from_objective(&"排查设备异常关机".repeat(20));
        assert!(cjk.title.chars().count() <= 73);
        assert!(cjk.title.ends_with('…'));
    }

    #[test]
    fn an_objective_with_no_sentence_still_yields_a_usable_title() {
        assert_eq!(GoalForm::from_objective("   ...  ").title, "Untitled goal");
    }

    #[test]
    fn a_new_goal_is_scheduled_from_creation_time() {
        let now = at("2026-08-21T00:00:00Z");
        let goal = GoalForm::default().into_new_goal(now).unwrap();
        assert!(goal.id.starts_with("goal_"));
        assert_eq!(goal.next_run_at, Some(at("2026-08-28T00:00:00Z")));
        assert_eq!(goal.next_source_check_at, Some(at("2026-08-22T00:00:00Z")));
    }

    #[test]
    fn editing_an_unrelated_field_does_not_postpone_due_work() {
        let created = at("2026-08-01T00:00:00Z");
        let mut goal = GoalForm::default().into_new_goal(created).unwrap();
        let was_due_at = goal.next_run_at;

        let mut form = GoalForm::from_goal(&goal);
        form.title = "Renamed".into();
        form.apply_to(&mut goal, at("2026-08-21T00:00:00Z")).unwrap();

        assert_eq!(goal.title, "Renamed");
        assert_eq!(goal.next_run_at, was_due_at);
    }

    #[test]
    fn changing_a_cadence_reschedules_only_that_turn() {
        let created = at("2026-08-01T00:00:00Z");
        let mut goal = GoalForm::default().into_new_goal(created).unwrap();
        let review_was_due_at = goal.next_review_at;

        let mut form = GoalForm::from_goal(&goal);
        form.cadence = Cadence::Daily;
        let now = at("2026-08-21T00:00:00Z");
        form.apply_to(&mut goal, now).unwrap();

        assert_eq!(goal.next_run_at, Some(at("2026-08-22T00:00:00Z")));
        assert_eq!(goal.next_review_at, review_was_due_at);
    }

    #[test]
    fn disabling_clears_the_schedule_and_re_enabling_rebuilds_it() {
        let created = at("2026-08-01T00:00:00Z");
        let mut goal = GoalForm::default().into_new_goal(created).unwrap();

        let mut form = GoalForm::from_goal(&goal);
        form.enabled = false;
        form.apply_to(&mut goal, at("2026-08-21T00:00:00Z")).unwrap();
        assert!(WorkKind::ALL.iter().all(|work| goal.next_at(*work).is_none()));

        let mut form = GoalForm::from_goal(&goal);
        form.enabled = true;
        let now = at("2026-08-22T00:00:00Z");
        form.apply_to(&mut goal, now).unwrap();
        assert_eq!(goal.next_run_at, Some(at("2026-08-29T00:00:00Z")));
    }

    #[test]
    fn each_turn_kind_gets_its_own_brief_and_the_policy_is_always_restated() {
        let goal = GoalForm::default().into_new_goal(at("2026-08-21T00:00:00Z")).unwrap();
        let learn = goal_prompt_for(&goal, WorkKind::Learn);
        let review = goal_prompt_for(&goal, WorkKind::Review);
        let summary = goal_prompt_for(&goal, WorkKind::Summary);

        assert!(learn.contains("Scheduled learning"));
        assert!(review.contains("Scheduled review"));
        assert!(summary.contains("Scheduled synthesis"));
        for brief in [&learn, &review, &summary] {
            assert!(brief.contains("never publish canonical knowledge without a human decision"));
            assert!(brief.contains("none specified"));
        }
    }
}
