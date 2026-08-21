//! Interpreting one unattended learning turn.
//!
//! The async plumbing that drives a runtime stream lives on `Engine`; the
//! decisions it makes along the way live here so they can be tested without a
//! runtime.

use methodus_domain::{AttentionKind, HumanAttention, PermissionDenial, WorkKind};

use crate::learning::attention::{parse_envelope, AttentionEnvelope};

/// Accumulates a runtime's streamed text into one canonical transcript.
#[derive(Debug, Default)]
pub struct TurnTranscript {
    output: String,
}

impl TurnTranscript {
    pub fn push_assistant(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if !self.output.is_empty() {
            self.output.push('\n');
        }
        self.output.push_str(text);
    }

    /// Fold in the terminal `result` text and take the transcript.
    ///
    /// Claude repeats the final assistant message inside the result envelope, so
    /// an exact repeat is dropped: CandidateSet extraction must see one copy.
    pub fn finish(mut self, result_text: &str) -> String {
        let result_text = result_text.trim_end();
        if !result_text.is_empty() && !self.output.trim_end().ends_with(result_text) {
            if !self.output.is_empty() {
                self.output.push('\n');
            }
            self.output.push_str(result_text);
        }
        self.output
    }
}

/// How an unattended turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnDisposition {
    /// The runtime failed. The run stays resumable.
    Failed { message: String },
    /// The turn stopped and needs a person before it can continue.
    AwaitingInput { envelope: Box<AttentionEnvelope> },
    /// The turn finished; its output may carry a CandidateSet.
    Completed,
}

/// Decide what a finished turn means.
///
/// A blocked tool call outranks a generic error flag: it is the specific,
/// actionable reason the turn stopped, and the runtime usually reports both.
pub fn classify(
    output: &str,
    is_error: bool,
    denials: &[PermissionDenial],
) -> TurnDisposition {
    if let Some(denial) = denials.first() {
        return TurnDisposition::AwaitingInput {
            envelope: Box::new(AttentionEnvelope {
                kind: AttentionKind::Permission,
                question: format!(
                    "Allow the runtime to use {} for this learning run?",
                    denial.tool_name
                ),
                context: Some("The runtime was blocked before a consequential tool action.".into()),
                tool_name: Some(denial.tool_name.clone()),
                tool_input: (!denial.tool_input.is_null())
                    .then(|| denial.tool_input.to_string()),
            }),
        };
    }
    if is_error {
        let message = output
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("the runtime reported an error without output");
        return TurnDisposition::Failed {
            message: message.to_string(),
        };
    }
    match parse_envelope(output) {
        Some(envelope) => TurnDisposition::AwaitingInput {
            envelope: Box::new(envelope),
        },
        None => TurnDisposition::Completed,
    }
}

/// What one unattended turn produced.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub run_id: String,
    pub goal_id: String,
    pub goal_title: String,
    pub work: WorkKind,
    pub candidate_ids: Vec<String>,
    pub attention: Option<HumanAttention>,
    pub failure: Option<String>,
    pub cost_usd: f64,
    /// Total spend for this Goal this month, after charging this turn.
    pub spent_usd: f64,
    pub budget_usd: f64,
}

impl TurnOutcome {
    pub fn budget_exhausted(&self) -> bool {
        self.spent_usd >= self.budget_usd
    }

    pub fn needs_attention(&self) -> bool {
        self.attention.is_some() || self.failure.is_some()
    }

    /// One sentence for the status bar and the OS notification.
    pub fn headline(&self) -> String {
        if let Some(message) = &self.failure {
            return format!("{} failed: {message}", self.goal_title);
        }
        if let Some(attention) = &self.attention {
            return format!("{} needs you — {}", self.goal_title, attention.title);
        }
        match self.candidate_ids.len() {
            0 => format!("{} finished with no new candidates", self.goal_title),
            1 => format!("{} produced 1 candidate for review", self.goal_title),
            count => format!("{} produced {count} candidates for review", self.goal_title),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Utc;
    use methodus_domain::AttentionStatus;

    fn denial(tool: &str) -> PermissionDenial {
        PermissionDenial {
            tool_name: tool.into(),
            tool_use_id: None,
            tool_input: serde_json::json!({ "command": "git push" }),
        }
    }

    #[test]
    fn assistant_chunks_are_joined_with_single_newlines() {
        let mut transcript = TurnTranscript::default();
        transcript.push_assistant("first");
        transcript.push_assistant("   ");
        transcript.push_assistant("second");
        assert_eq!(transcript.finish(""), "first\nsecond");
    }

    #[test]
    fn a_repeated_result_text_is_not_duplicated() {
        let mut transcript = TurnTranscript::default();
        transcript.push_assistant("the final synthesis");
        assert_eq!(transcript.finish("the final synthesis"), "the final synthesis");
    }

    #[test]
    fn a_result_that_adds_new_text_is_appended() {
        let mut transcript = TurnTranscript::default();
        transcript.push_assistant("partial");
        assert_eq!(transcript.finish("conclusion"), "partial\nconclusion");
    }

    #[test]
    fn a_result_arriving_without_any_assistant_text_still_lands() {
        assert_eq!(TurnTranscript::default().finish("only result"), "only result");
    }

    #[test]
    fn a_blocked_tool_outranks_the_error_flag() {
        let disposition = classify("something went wrong", true, &[denial("Bash")]);
        let TurnDisposition::AwaitingInput { envelope } = disposition else {
            panic!("expected a permission hand-off");
        };
        assert_eq!(envelope.kind, AttentionKind::Permission);
        assert_eq!(envelope.tool_name.as_deref(), Some("Bash"));
        assert!(envelope.tool_input.unwrap().contains("git push"));
    }

    #[test]
    fn an_error_result_reports_the_first_meaningful_line() {
        let disposition = classify("\n\n  rate limit exceeded\ndetails follow", true, &[]);
        assert_eq!(
            disposition,
            TurnDisposition::Failed { message: "rate limit exceeded".into() }
        );
    }

    #[test]
    fn an_error_without_output_still_produces_a_message() {
        let TurnDisposition::Failed { message } = classify("   ", true, &[]) else {
            panic!("expected a failure");
        };
        assert!(!message.is_empty());
    }

    #[test]
    fn an_attention_envelope_in_the_output_stops_the_turn() {
        let output = "```json\n{\"outcome\":\"needs_input\",\"question\":\"Which source wins?\"}\n```";
        let TurnDisposition::AwaitingInput { envelope } = classify(output, false, &[]) else {
            panic!("expected a question hand-off");
        };
        assert_eq!(envelope.question, "Which source wins?");
    }

    #[test]
    fn a_plain_successful_turn_completes() {
        assert_eq!(
            classify("Here is what I learned.", false, &[]),
            TurnDisposition::Completed
        );
    }

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

    #[test]
    fn the_headline_names_the_goal_and_what_happened() {
        let mut done = outcome();
        assert!(done.headline().contains("no new candidates"));

        done.candidate_ids = vec!["knowledge/candidate-1".into()];
        assert!(done.headline().contains("1 candidate"));
        done.candidate_ids.push("knowledge/candidate-2".into());
        assert!(done.headline().contains("2 candidates"));
        assert!(!done.needs_attention());
    }

    #[test]
    fn a_failure_or_hand_off_takes_over_the_headline() {
        let mut blocked = outcome();
        blocked.candidate_ids = vec!["knowledge/candidate-1".into()];
        blocked.attention = Some(HumanAttention {
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
        });
        assert!(blocked.headline().contains("needs you"));
        assert!(blocked.needs_attention());

        blocked.failure = Some("runtime exited".into());
        assert!(blocked.headline().contains("failed: runtime exited"));
    }

    #[test]
    fn budget_exhaustion_is_reported_from_the_charged_total() {
        let mut spent = outcome();
        assert!(!spent.budget_exhausted());
        spent.spent_usd = 20.0;
        assert!(spent.budget_exhausted());
    }
}
