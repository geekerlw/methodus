//! Continuous-learning records shared by every Methodus surface.
//!
//! A [`LearningGoal`] is the durable human intent above individual Learn runs:
//! the investigation prompt, its authorized sources, four independent cadences,
//! a monthly budget, and the review policy. Cadence arithmetic and quiet-hour
//! evaluation live here so no surface reimplements them.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, NaiveTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::DomainError;

fn invalid(entity: &'static str, value: &str) -> DomainError {
    DomainError::InvalidStatus {
        entity,
        value: value.to_string(),
    }
}

// ─── Cadence ─────────────────────────────────────────────────────────────────

/// How often Methodus schedules one kind of automatic turn.
///
/// Serialized as a human-editable string so the same value round-trips through
/// SQLite and the `$EDITOR` Goal form: `manual`, `daily`, `weekly`, `monthly`,
/// or `every:<hours>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Manual,
    Daily,
    Weekly,
    Monthly,
    EveryHours(u32),
}

impl Cadence {
    /// The next occurrence after `now`, or `None` when the cadence is manual.
    pub fn next_after(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let delta = match self {
            Self::Manual => return None,
            Self::Daily => Duration::days(1),
            Self::Weekly => Duration::weeks(1),
            Self::Monthly => Duration::days(30),
            Self::EveryHours(hours) => Duration::hours(i64::from(hours.max(1))),
        };
        Some(now + delta)
    }

    pub fn is_manual(self) -> bool {
        matches!(self, Self::Manual)
    }
}

impl fmt::Display for Cadence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manual => formatter.write_str("manual"),
            Self::Daily => formatter.write_str("daily"),
            Self::Weekly => formatter.write_str("weekly"),
            Self::Monthly => formatter.write_str("monthly"),
            Self::EveryHours(hours) => write!(formatter, "every:{hours}"),
        }
    }
}

impl FromStr for Cadence {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim().to_ascii_lowercase();
        if let Some(hours) = value.strip_prefix("every:") {
            return hours
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|hours| *hours > 0)
                .map(Self::EveryHours)
                .ok_or_else(|| invalid("cadence", &value));
        }
        match value.as_str() {
            // `once`/`off`/`disabled` stay accepted so goals authored against the
            // earlier prototype vocabulary keep loading.
            "manual" | "once" | "off" | "disabled" => Ok(Self::Manual),
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            _ => Err(invalid("cadence", &value)),
        }
    }
}

impl Serialize for Cadence {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Cadence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

// ─── Quiet hours ─────────────────────────────────────────────────────────────

/// A daily local-time window during which automatic work is deferred.
///
/// Both ends are always present, so "start set but end missing" is
/// unrepresentable instead of being rejected by a separate validation pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl QuietHours {
    pub fn new(start: NaiveTime, end: NaiveTime) -> Self {
        Self { start, end }
    }

    /// Parse an `HH:MM` pair.
    pub fn parse(start: &str, end: &str) -> Result<Self, DomainError> {
        Ok(Self {
            start: parse_hhmm(start)?,
            end: parse_hhmm(end)?,
        })
    }

    /// Whether `at` (local wall-clock time) falls inside the window. Equal ends
    /// mean "always quiet"; a window crossing midnight is handled explicitly.
    pub fn contains(&self, at: NaiveTime) -> bool {
        if self.start == self.end {
            return true;
        }
        if self.start < self.end {
            at >= self.start && at < self.end
        } else {
            at >= self.start || at < self.end
        }
    }
}

impl fmt::Display for QuietHours {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}–{}", format_hhmm(self.start), format_hhmm(self.end))
    }
}

fn parse_hhmm(value: &str) -> Result<NaiveTime, DomainError> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M").map_err(|_| invalid("quiet hours", value))
}

fn format_hhmm(value: NaiveTime) -> String {
    value.format("%H:%M").to_string()
}

#[derive(Serialize, Deserialize)]
struct QuietHoursRepr {
    start: String,
    end: String,
}

impl Serialize for QuietHours {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        QuietHoursRepr {
            start: format_hhmm(self.start),
            end: format_hhmm(self.end),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for QuietHours {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = QuietHoursRepr::deserialize(deserializer)?;
        Self::parse(&repr.start, &repr.end).map_err(serde::de::Error::custom)
    }
}

// ─── Work kind ───────────────────────────────────────────────────────────────

/// One kind of scheduled turn against a Goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Learn,
    Review,
    Summary,
    SourceCheck,
}

impl WorkKind {
    /// Runtime-occupying turns in the order the scheduler prefers them.
    pub const RUNTIME_TURNS: [Self; 3] = [Self::Learn, Self::Review, Self::Summary];
    pub const ALL: [Self; 4] = [Self::Learn, Self::Review, Self::Summary, Self::SourceCheck];

    /// Whether this turn occupies the Goal's executor session. A source check is
    /// an index-only pass and never launches a runtime.
    pub fn needs_runtime(self) -> bool {
        !matches!(self, Self::SourceCheck)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Learn => "learn",
            Self::Review => "review",
            Self::Summary => "summary",
            Self::SourceCheck => "source_check",
        }
    }
}

impl fmt::Display for WorkKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WorkKind {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "learn" => Ok(Self::Learn),
            "review" => Ok(Self::Review),
            "summary" => Ok(Self::Summary),
            "source_check" => Ok(Self::SourceCheck),
            other => Err(invalid("work kind", other)),
        }
    }
}

// ─── Review policy ───────────────────────────────────────────────────────────

/// How a Goal's candidate output reaches canonical status.
///
/// There is deliberately no automatic-publication variant: "only human Review
/// may establish canonical knowledge" is a type-level guarantee, not a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPolicy {
    /// Every candidate waits for an explicit maintainer decision.
    HumanRequired,
    /// As above, and the runtime is additionally told to raise open questions.
    MaintainerQuestions,
}

impl ReviewPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HumanRequired => "human_required",
            Self::MaintainerQuestions => "maintainer_questions",
        }
    }
}

impl fmt::Display for ReviewPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ReviewPolicy {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "human_required" => Ok(Self::HumanRequired),
            "maintainer_questions" => Ok(Self::MaintainerQuestions),
            other => Err(invalid("review policy", other)),
        }
    }
}

// ─── Learning goal ───────────────────────────────────────────────────────────

/// A long-lived learning objective with its own schedule, budget, and policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearningGoal {
    pub id: String,
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
    #[serde(default)]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub next_review_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub next_summary_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub next_source_check_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LearningGoal {
    pub fn cadence_for(&self, work: WorkKind) -> Cadence {
        match work {
            WorkKind::Learn => self.cadence,
            WorkKind::Review => self.review_cadence,
            WorkKind::Summary => self.summary_cadence,
            WorkKind::SourceCheck => self.source_check_cadence,
        }
    }

    pub fn next_at(&self, work: WorkKind) -> Option<DateTime<Utc>> {
        match work {
            WorkKind::Learn => self.next_run_at,
            WorkKind::Review => self.next_review_at,
            WorkKind::Summary => self.next_summary_at,
            WorkKind::SourceCheck => self.next_source_check_at,
        }
    }

    pub fn set_next_at(&mut self, work: WorkKind, at: Option<DateTime<Utc>>) {
        match work {
            WorkKind::Learn => self.next_run_at = at,
            WorkKind::Review => self.next_review_at = at,
            WorkKind::Summary => self.next_summary_at = at,
            WorkKind::SourceCheck => self.next_source_check_at = at,
        }
    }

    /// Advance one cadence past `now` after that turn has been dispatched.
    pub fn advance(&mut self, work: WorkKind, now: DateTime<Utc>) {
        let next = self.cadence_for(work).next_after(now);
        self.set_next_at(work, next);
    }

    /// Recompute every schedule. A disabled Goal holds no due timestamps.
    pub fn reschedule_all(&mut self, now: DateTime<Utc>) {
        for work in WorkKind::ALL {
            let next = self.enabled.then(|| self.cadence_for(work).next_after(now)).flatten();
            self.set_next_at(work, next);
        }
    }

    pub fn is_due(&self, work: WorkKind, now: DateTime<Utc>) -> bool {
        self.enabled && self.next_at(work).is_some_and(|at| at <= now)
    }

    /// Whether automatic work should be deferred at this local wall-clock time.
    /// Deferral deliberately leaves the due timestamp untouched so the turn runs
    /// as soon as the window closes.
    pub fn is_quiet_at(&self, local_time: NaiveTime) -> bool {
        self.quiet_hours
            .is_some_and(|window| window.contains(local_time))
    }
}

// ─── Human attention ─────────────────────────────────────────────────────────

/// Why a run stopped and handed control back to a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// The runtime needs a maintainer judgment before it can continue.
    Question,
    /// The runtime needs a bounded permission it does not currently hold.
    Permission,
}

impl AttentionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Permission => "permission",
        }
    }
}

impl fmt::Display for AttentionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AttentionKind {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "question" => Ok(Self::Question),
            "permission" => Ok(Self::Permission),
            other => Err(invalid("attention kind", other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionStatus {
    Open,
    Resolved,
}

impl AttentionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }
}

impl fmt::Display for AttentionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AttentionStatus {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            other => Err(invalid("attention status", other)),
        }
    }
}

/// A durable hand-off between a runtime and its maintainer.
///
/// `goal_id` is denormalized so the scheduler can tell which Goals are blocked
/// on a person without walking run links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanAttention {
    pub id: String,
    pub run_id: String,
    #[serde(default)]
    pub goal_id: Option<String>,
    pub kind: AttentionKind,
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<String>,
    pub status: AttentionStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub response: Option<String>,
}

impl HumanAttention {
    pub fn is_open(&self) -> bool {
        self.status == AttentionStatus::Open
    }
}

// ─── Budget accounting ───────────────────────────────────────────────────────

/// Calendar month key (`YYYY-MM`) used to roll per-Goal spend.
pub fn usage_month(at: DateTime<Utc>) -> String {
    at.format("%Y-%m").to_string()
}

/// One Goal's spend within one calendar month. Past months are retained rather
/// than reset in place, so spend history stays inspectable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalUsage {
    pub goal_id: String,
    pub month: String,
    pub spent_usd: f64,
    pub updated_at: DateTime<Utc>,
}

impl GoalUsage {
    pub fn exhausts(&self, budget_usd: f64) -> bool {
        self.spent_usd >= budget_usd
    }
}

/// Links a Learn run back to the Goal and the kind of turn that started it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalRun {
    pub run_id: String,
    pub goal_id: String,
    pub work: WorkKind,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
    }

    fn time(value: &str) -> NaiveTime {
        NaiveTime::parse_from_str(value, "%H:%M").unwrap()
    }

    #[test]
    fn cadence_round_trips_through_its_string_form() {
        for (text, cadence) in [
            ("manual", Cadence::Manual),
            ("daily", Cadence::Daily),
            ("weekly", Cadence::Weekly),
            ("monthly", Cadence::Monthly),
            ("every:6", Cadence::EveryHours(6)),
        ] {
            assert_eq!(text.parse::<Cadence>().unwrap(), cadence);
            assert_eq!(cadence.to_string(), text);
        }
        // Retired prototype vocabulary still loads.
        assert_eq!("off".parse::<Cadence>().unwrap(), Cadence::Manual);
        assert_eq!("once".parse::<Cadence>().unwrap(), Cadence::Manual);
        assert!("every:0".parse::<Cadence>().is_err());
        assert!("hourly".parse::<Cadence>().is_err());
    }

    #[test]
    fn cadence_arithmetic_is_deterministic_and_manual_never_schedules() {
        let now = at("2026-08-21T00:00:00Z");
        assert_eq!(Cadence::Daily.next_after(now), Some(at("2026-08-22T00:00:00Z")));
        assert_eq!(Cadence::EveryHours(6).next_after(now), Some(at("2026-08-21T06:00:00Z")));
        assert_eq!(Cadence::Manual.next_after(now), None);
    }

    #[test]
    fn quiet_hours_handle_midnight_wrap_and_full_day() {
        let overnight = QuietHours::parse("22:00", "07:00").unwrap();
        assert!(overnight.contains(time("23:30")));
        assert!(overnight.contains(time("06:59")));
        assert!(!overnight.contains(time("07:00")));
        assert!(!overnight.contains(time("12:00")));

        let daytime = QuietHours::parse("09:00", "17:00").unwrap();
        assert!(daytime.contains(time("09:00")));
        assert!(!daytime.contains(time("17:00")));

        assert!(QuietHours::parse("08:00", "08:00").unwrap().contains(time("03:00")));
        assert!(QuietHours::parse("25:00", "07:00").is_err());
    }

    #[test]
    fn quiet_hours_serialize_as_editable_hhmm() {
        let window = QuietHours::parse("22:00", "07:00").unwrap();
        let encoded = serde_json::to_string(&window).unwrap();
        assert_eq!(encoded, r#"{"start":"22:00","end":"07:00"}"#);
        assert_eq!(serde_json::from_str::<QuietHours>(&encoded).unwrap(), window);
    }

    fn goal(now: DateTime<Utc>) -> LearningGoal {
        LearningGoal {
            id: "goal_test".into(),
            title: "Shutdown recovery".into(),
            prompt: "Understand the recovery path".into(),
            sources: vec!["docs/runbook".into()],
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
            next_run_at: None,
            next_review_at: None,
            next_summary_at: None,
            next_source_check_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn rescheduling_sets_every_cadence_and_disabling_clears_them() {
        let now = at("2026-08-21T00:00:00Z");
        let mut goal = goal(now);
        goal.reschedule_all(now);
        assert_eq!(goal.next_run_at, Some(at("2026-08-28T00:00:00Z")));
        assert_eq!(goal.next_source_check_at, Some(at("2026-08-22T00:00:00Z")));

        goal.enabled = false;
        goal.reschedule_all(now);
        assert!(WorkKind::ALL.iter().all(|work| goal.next_at(*work).is_none()));
    }

    #[test]
    fn a_manual_cadence_never_becomes_due_but_others_do() {
        let now = at("2026-08-21T00:00:00Z");
        let mut goal = goal(now);
        goal.cadence = Cadence::Manual;
        goal.reschedule_all(now);
        assert!(!goal.is_due(WorkKind::Learn, at("2027-01-01T00:00:00Z")));

        goal.advance(WorkKind::SourceCheck, now);
        assert!(!goal.is_due(WorkKind::SourceCheck, now));
        assert!(goal.is_due(WorkKind::SourceCheck, at("2026-08-22T00:01:00Z")));
    }

    #[test]
    fn a_disabled_goal_is_never_due_even_with_a_stale_timestamp() {
        let now = at("2026-08-21T00:00:00Z");
        let mut goal = goal(now);
        goal.reschedule_all(now);
        goal.enabled = false;
        assert!(!goal.is_due(WorkKind::Learn, at("2027-01-01T00:00:00Z")));
    }

    #[test]
    fn source_check_is_the_only_turn_that_needs_no_runtime() {
        assert!(!WorkKind::SourceCheck.needs_runtime());
        assert!(WorkKind::RUNTIME_TURNS.iter().all(|work| work.needs_runtime()));
    }

    #[test]
    fn usage_month_keys_are_calendar_scoped() {
        assert_eq!(usage_month(at("2026-08-21T23:59:00Z")), "2026-08");
        let usage = GoalUsage {
            goal_id: "goal_test".into(),
            month: "2026-08".into(),
            spent_usd: 20.0,
            updated_at: at("2026-08-21T00:00:00Z"),
        };
        assert!(usage.exhausts(20.0));
        assert!(!usage.exhausts(20.01));
    }
}
