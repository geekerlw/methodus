use serde::{Deserialize, Serialize};

/// Durable app-facing hand-off between a runtime and its maintainer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HumanAttention {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) context: Option<String>,
    #[serde(default)]
    pub(crate) tool_name: Option<String>,
    #[serde(default)]
    pub(crate) tool_input: Option<String>,
    pub(crate) status: String,
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) resolved_at: Option<String>,
    #[serde(default)]
    pub(crate) response: Option<String>,
}

pub(crate) type ParsedAttention = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Parse only the explicit attention envelope. CandidateSet JSON must never be
/// mistaken for a question, so the `outcome` discriminator is mandatory.
pub(crate) fn parse_structured(output: &str) -> Option<ParsedAttention> {
    let mut blocks = Vec::new();
    let mut in_fence = false;
    let mut current = String::new();
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
    if !current.trim().is_empty() {
        blocks.push(current);
    }
    if let (Some(start), Some(end)) = (output.find('{'), output.rfind('}')) {
        if start < end {
            blocks.push(output[start..=end].into());
        }
    }
    blocks.into_iter().find_map(|block| {
        let value: serde_json::Value = serde_json::from_str(block.trim()).ok()?;
        let outcome = value.get("outcome")?.as_str()?;
        if !matches!(outcome, "needs_input" | "permission_required") {
            return None;
        }
        let question = value
            .get("question")
            .and_then(|value| value.as_str())
            .filter(|question| !question.trim().is_empty())
            .unwrap_or("The runtime needs a maintainer decision before it can continue.")
            .to_string();
        let context = value.get("context").and_then(|value| value.as_str()).map(str::to_string);
        let tool_name = value.get("tool_name").and_then(|value| value.as_str()).map(str::to_string);
        let tool_input = value.get("tool_input").map(|value| value.to_string());
        Some((outcome.to_string(), question, context, tool_name, tool_input))
    })
}
