//! Executor token / cost rollup parsed from `RuntimeEvent::Result`.

use serde::{Deserialize, Serialize};

/// One turn's usage as reported by Claude / Codex / Cursor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageDelta {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
}

impl UsageDelta {
    pub fn from_result(cost_usd: Option<f64>, usage: Option<&serde_json::Value>) -> Self {
        let mut out = Self {
            cost_usd: cost_usd.filter(|c| c.is_finite() && *c > 0.0),
            ..Self::default()
        };
        let Some(usage) = usage else {
            return out;
        };
        out.input_tokens = json_i64(
            usage,
            &[
                "input_tokens",
                "inputTokens",
                "prompt_tokens",
                "promptTokens",
            ],
        );
        out.output_tokens = json_i64(
            usage,
            &[
                "output_tokens",
                "outputTokens",
                "completion_tokens",
                "completionTokens",
            ],
        );
        out.cache_read_tokens = json_i64(
            usage,
            &[
                "cache_read_input_tokens",
                "cache_read_tokens",
                "cacheReadTokens",
            ],
        );
        out.cache_write_tokens = json_i64(
            usage,
            &[
                "cache_creation_input_tokens",
                "cache_creation_tokens",
                "cacheWriteTokens",
            ],
        );
        if out.cost_usd.is_none() {
            out.cost_usd = usage
                .get("cost_usd")
                .or_else(|| usage.get("total_cost_usd"))
                .and_then(|v| v.as_f64())
                .filter(|c| c.is_finite() && *c > 0.0);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_write_tokens == 0
            && self.cost_usd.is_none()
    }

    pub fn compact(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        if self.input_tokens > 0 || self.output_tokens > 0 {
            parts.push(format!(
                "{}↓ {}↑",
                fmt_tokens(self.input_tokens),
                fmt_tokens(self.output_tokens)
            ));
        }
        if self.cache_read_tokens > 0 {
            parts.push(format!("cache {}", fmt_tokens(self.cache_read_tokens)));
        }
        if let Some(c) = self.cost_usd {
            parts.push(format!("${c:.3}"));
        }
        parts.join("  ")
    }
}

/// Aggregated usage across turns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageSummary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
    pub turns: i64,
}

impl UsageSummary {
    pub fn compact(&self) -> String {
        if self.turns == 0 && self.input_tokens == 0 {
            return "no usage yet".to_string();
        }
        let mut s = format!(
            "{}↓ {}↑",
            fmt_tokens(self.input_tokens),
            fmt_tokens(self.output_tokens)
        );
        if self.cache_read_tokens > 0 {
            s.push_str(&format!("  cache {}", fmt_tokens(self.cache_read_tokens)));
        }
        if self.cost_usd > 0.0 {
            s.push_str(&format!("  ${:.3}", self.cost_usd));
        }
        s.push_str(&format!(
            "  {} turn{}",
            self.turns,
            if self.turns == 1 { "" } else { "s" }
        ));
        s
    }
}

pub fn fmt_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}m", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

fn json_i64(value: &serde_json::Value, keys: &[&str]) -> i64 {
    for key in keys {
        if let Some(v) = value.get(*key) {
            if let Some(i) = v.as_i64() {
                return i.max(0);
            }
            if let Some(u) = v.as_u64() {
                return u.min(i64::MAX as u64) as i64;
            }
            if let Some(f) = v.as_f64() {
                return f.max(0.0) as i64;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_usage() {
        let usage = serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 2000,
            "cache_creation_input_tokens": 10
        });
        let d = UsageDelta::from_result(Some(0.05), Some(&usage));
        assert_eq!(d.input_tokens, 100);
        assert_eq!(d.output_tokens, 50);
        assert_eq!(d.cache_read_tokens, 2000);
        assert_eq!(d.cache_write_tokens, 10);
        assert_eq!(d.cost_usd, Some(0.05));
        let s = d.compact();
        assert!(s.contains("100↓"));
        assert!(s.contains("$0.050"));
    }

    #[test]
    fn empty_when_missing() {
        assert!(UsageDelta::from_result(None, None).is_empty());
    }
}
