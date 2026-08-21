//! Detecting when a runtime has handed control back to a person.

use methodus_domain::AttentionKind;

/// A parsed attention envelope emitted by a runtime turn.
#[derive(Debug, Clone, PartialEq)]
pub struct AttentionEnvelope {
    pub kind: AttentionKind,
    pub question: String,
    pub context: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
}

const DEFAULT_QUESTION: &str =
    "The runtime needs a maintainer decision before it can continue.";

/// Parse only an explicit attention envelope from a turn's output.
///
/// The `outcome` discriminator is mandatory: a CandidateSet is also JSON, and
/// mistaking one for a question would stall a Goal on a run that in fact
/// finished. Fenced blocks are checked before the whole-output fallback so a
/// deliberate envelope always wins over incidental braces in prose.
pub fn parse_envelope(output: &str) -> Option<AttentionEnvelope> {
    candidate_blocks(output).into_iter().find_map(|block| {
        let value: serde_json::Value = serde_json::from_str(block.trim()).ok()?;
        let kind = match value.get("outcome")?.as_str()? {
            "needs_input" => AttentionKind::Question,
            "permission_required" => AttentionKind::Permission,
            _ => return None,
        };
        let question = value
            .get("question")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|question| !question.is_empty())
            .unwrap_or(DEFAULT_QUESTION)
            .to_string();
        Some(AttentionEnvelope {
            kind,
            question,
            context: string_field(&value, "context"),
            tool_name: string_field(&value, "tool_name"),
            tool_input: value.get("tool_input").map(ToString::to_string),
        })
    })
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn candidate_blocks(output: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    let mut in_fence = false;
    for line in output.lines() {
        if line.trim_start().starts_with("```") {
            if in_fence {
                blocks.push(std::mem::take(&mut current));
            }
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            current.push_str(line);
            current.push('\n');
        }
    }
    // An unterminated fence still carries a usable payload.
    if !current.trim().is_empty() {
        blocks.push(current);
    }
    if let (Some(start), Some(end)) = (output.find('{'), output.rfind('}')) {
        if start < end {
            blocks.push(output[start..=end].to_string());
        }
    }
    blocks
}

/// A short headline for the attention queue, derived from the question itself.
pub fn envelope_title(envelope: &AttentionEnvelope) -> String {
    match envelope.kind {
        AttentionKind::Permission => envelope
            .tool_name
            .as_ref()
            .map(|tool| format!("Permission needed: {tool}"))
            .unwrap_or_else(|| "Permission needed".to_string()),
        AttentionKind::Question => first_line(&envelope.question, 80),
    }
}

fn first_line(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= limit {
        return line.to_string();
    }
    let truncated: String = line.chars().take(limit.saturating_sub(1)).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fenced_needs_input_envelope_is_parsed() {
        let output = "Here is what I found.\n\n```json\n{\
            \"outcome\": \"needs_input\", \
            \"question\": \"Which retry policy is authoritative?\", \
            \"context\": \"docs/runbook#retry\"}\n```\n";
        let envelope = parse_envelope(output).unwrap();
        assert_eq!(envelope.kind, AttentionKind::Question);
        assert_eq!(envelope.question, "Which retry policy is authoritative?");
        assert_eq!(envelope.context.as_deref(), Some("docs/runbook#retry"));
    }

    #[test]
    fn a_permission_envelope_carries_the_tool_and_its_input() {
        let output = "```\n{\"outcome\":\"permission_required\",\"tool_name\":\"Bash\",\
            \"tool_input\":{\"command\":\"git push\"}}\n```";
        let envelope = parse_envelope(output).unwrap();
        assert_eq!(envelope.kind, AttentionKind::Permission);
        assert_eq!(envelope.tool_name.as_deref(), Some("Bash"));
        assert!(envelope.tool_input.as_deref().unwrap().contains("git push"));
        assert_eq!(envelope.question, DEFAULT_QUESTION);
        assert_eq!(envelope_title(&envelope), "Permission needed: Bash");
    }

    #[test]
    fn a_candidate_set_is_never_mistaken_for_a_question() {
        let output = "```json\n{\"candidates\":[{\"title\":\"Retry policy\",\
            \"question\":\"why?\"}]}\n```";
        assert!(parse_envelope(output).is_none());
    }

    #[test]
    fn an_unknown_outcome_is_ignored() {
        let output = "{\"outcome\":\"completed\",\"question\":\"done?\"}";
        assert!(parse_envelope(output).is_none());
    }

    #[test]
    fn a_bare_envelope_without_a_fence_is_still_found() {
        let output = "I need a decision. {\"outcome\":\"needs_input\",\
            \"question\":\"Ship it?\"} Thanks.";
        assert_eq!(parse_envelope(output).unwrap().question, "Ship it?");
    }

    #[test]
    fn a_deliberate_fenced_envelope_wins_over_incidental_braces() {
        let output = "The config {\"a\": 1} is unclear.\n\n```json\n\
            {\"outcome\":\"needs_input\",\"question\":\"Which config wins?\"}\n```\n";
        assert_eq!(
            parse_envelope(output).unwrap().question,
            "Which config wins?"
        );
    }

    #[test]
    fn output_without_any_envelope_yields_nothing() {
        assert!(parse_envelope("A plain prose answer with no JSON at all.").is_none());
        assert!(parse_envelope("").is_none());
    }

    #[test]
    fn a_long_question_is_truncated_for_the_queue_headline() {
        let envelope = AttentionEnvelope {
            kind: AttentionKind::Question,
            question: format!("{}\nsecond line", "x".repeat(200)),
            context: None,
            tool_name: None,
            tool_input: None,
        };
        let title = envelope_title(&envelope);
        assert_eq!(title.chars().count(), 80);
        assert!(title.ends_with('…'));
    }
}
