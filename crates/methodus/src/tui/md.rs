//! Lightweight markdown for the agent transcript. Not a full CommonMark parser:
//! headings, fences, lists, **bold**, `code`, and diff markers inside fences.

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdStyle {
    Body,
    Dim,
    Bold,
    Italic,
    Heading,
    Code,
    Fence,
    DiffAdd,
    DiffDel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdSpan {
    pub text: String,
    pub style: MdStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdLine {
    pub spans: Vec<MdSpan>,
    pub pad: usize,
}

/// Render markdown into display rows wrapped to `width` columns.
pub fn render_md(src: &str, width: usize) -> Vec<MdLine> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut fence_lang = String::new();
    for raw in src.split('\n') {
        if let Some(lang) = fence_open(raw) {
            in_fence = !in_fence;
            if in_fence {
                fence_lang = lang;
                out.push(plain(
                    format!("```{fence_lang}"),
                    MdStyle::Dim,
                    0,
                    width,
                ));
            } else {
                fence_lang.clear();
                out.push(plain("```".to_string(), MdStyle::Dim, 0, width));
            }
            continue;
        }
        if in_fence {
            let style = if raw.starts_with('+') && !raw.starts_with("+++") {
                MdStyle::DiffAdd
            } else if raw.starts_with('-') && !raw.starts_with("---") {
                MdStyle::DiffDel
            } else {
                MdStyle::Fence
            };
            out.extend(wrap_styled(raw, style, 2, width));
            continue;
        }
        if let Some(rest) = heading(raw) {
            out.extend(wrap_styled(rest, MdStyle::Heading, 0, width));
            continue;
        }
        if let Some(rest) = list_item(raw) {
            let mut first = true;
            for mut line in wrap_inline(rest, width.saturating_sub(2).max(8)) {
                if first {
                    line.pad = 0;
                    if let Some(span) = line.spans.first_mut() {
                        span.text = format!("· {}", span.text);
                    } else {
                        line.spans.push(MdSpan {
                            text: "· ".to_string(),
                            style: MdStyle::Dim,
                        });
                    }
                    first = false;
                } else {
                    line.pad = 2;
                }
                out.push(line);
            }
            continue;
        }
        if raw.trim().is_empty() {
            out.push(MdLine {
                spans: Vec::new(),
                pad: 0,
            });
            continue;
        }
        out.extend(wrap_inline(raw, width.max(8)));
    }
    if out.is_empty() {
        out.push(MdLine {
            spans: Vec::new(),
            pad: 0,
        });
    }
    out
}

fn fence_open(line: &str) -> Option<String> {
    let t = line.trim_start();
    t.strip_prefix("```").map(|rest| rest.trim().to_string())
}

fn heading(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let rest = t.strip_prefix('#')?;
    let rest = rest.trim_start_matches('#');
    let rest = rest.strip_prefix(' ')?;
    Some(rest.trim())
}

fn list_item(line: &str) -> Option<&str> {
    let t = line.trim_start();
    t.strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| {
            let mut chars = t.chars();
            if chars.next()?.is_ascii_digit() && matches!(chars.next(), Some('.') | Some(')')) {
                Some(t.split_once(' ')?.1)
            } else {
                None
            }
        })
}

fn plain(text: String, style: MdStyle, pad: usize, width: usize) -> MdLine {
    wrap_styled(&text, style, pad, width)
        .into_iter()
        .next()
        .unwrap_or(MdLine {
            spans: vec![MdSpan { text, style }],
            pad,
        })
}

fn wrap_styled(text: &str, style: MdStyle, pad: usize, width: usize) -> Vec<MdLine> {
    let inner = width.saturating_sub(pad).max(8);
    wrap_text_cols(text, inner)
        .into_iter()
        .map(|t| MdLine {
            spans: if t.is_empty() {
                Vec::new()
            } else {
                vec![MdSpan { text: t, style }]
            },
            pad,
        })
        .collect()
}

fn wrap_inline(text: &str, width: usize) -> Vec<MdLine> {
    let spans = parse_inline(text);
    wrap_spans(spans, width.max(8))
}

fn parse_inline(text: &str) -> Vec<MdSpan> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<MdSpan>, style: MdStyle| {
        if !buf.is_empty() {
            out.push(MdSpan {
                text: std::mem::take(buf),
                style,
            });
        }
    };
    while i < chars.len() {
        if chars[i] == '`' {
            flush(&mut buf, &mut out, MdStyle::Body);
            i += 1;
            let mut code = String::new();
            while i < chars.len() && chars[i] != '`' {
                code.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            if !code.is_empty() {
                out.push(MdSpan {
                    text: code,
                    style: MdStyle::Code,
                });
            }
            continue;
        }
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            flush(&mut buf, &mut out, MdStyle::Body);
            i += 2;
            let mut bold = String::new();
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '*') {
                bold.push(chars[i]);
                i += 1;
            }
            if i + 1 < chars.len() {
                i += 2;
            }
            if !bold.is_empty() {
                out.push(MdSpan {
                    text: bold,
                    style: MdStyle::Bold,
                });
            }
            continue;
        }
        if chars[i] == '*' {
            flush(&mut buf, &mut out, MdStyle::Body);
            i += 1;
            let mut italic = String::new();
            while i < chars.len() && chars[i] != '*' {
                italic.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            if !italic.is_empty() {
                out.push(MdSpan {
                    text: italic,
                    style: MdStyle::Italic,
                });
            }
            continue;
        }
        buf.push(chars[i]);
        i += 1;
    }
    flush(&mut buf, &mut out, MdStyle::Body);
    if out.is_empty() {
        out.push(MdSpan {
            text: text.to_string(),
            style: MdStyle::Body,
        });
    }
    out
}

fn wrap_spans(spans: Vec<MdSpan>, width: usize) -> Vec<MdLine> {
    let mut lines = Vec::new();
    let mut cur: Vec<MdSpan> = Vec::new();
    let mut cols = 0usize;
    for span in spans {
        for ch in span.text.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cols > 0 && cols + w > width {
                lines.push(MdLine {
                    spans: std::mem::take(&mut cur),
                    pad: 0,
                });
                cols = 0;
            }
            if let Some(last) = cur.last_mut() {
                if last.style == span.style {
                    last.text.push(ch);
                } else {
                    cur.push(MdSpan {
                        text: ch.to_string(),
                        style: span.style,
                    });
                }
            } else {
                cur.push(MdSpan {
                    text: ch.to_string(),
                    style: span.style,
                });
            }
            cols = cols.saturating_add(w);
        }
    }
    lines.push(MdLine { spans: cur, pad: 0 });
    if lines.is_empty() {
        lines.push(MdLine {
            spans: Vec::new(),
            pad: 0,
        });
    }
    lines
}

fn wrap_text_cols(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut line = String::new();
    let mut cols = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols > 0 && cols + w > width {
            out.push(std::mem::take(&mut line));
            cols = 0;
        }
        line.push(ch);
        cols = cols.saturating_add(w);
    }
    out.push(line);
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(lines: &[MdLine]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn headings_and_bold_and_code() {
        let lines = render_md("## Hello **world** and `x`\n", 40);
        let joined = dump(&lines).join(" ");
        assert!(joined.contains("Hello"));
        assert!(lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.style == MdStyle::Heading || s.style == MdStyle::Bold || s.style == MdStyle::Code));
    }

    #[test]
    fn fence_diff_markers() {
        let src = "```diff\n+added\n-removed\n stay\n```\n";
        let lines = render_md(src, 40);
        assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.style == MdStyle::DiffAdd)));
        assert!(lines.iter().any(|l| l.spans.iter().any(|s| s.style == MdStyle::DiffDel)));
    }

    #[test]
    fn list_items_get_dot() {
        let lines = render_md("- alpha\n- beta", 40);
        let text = dump(&lines).join("\n");
        assert!(text.contains("· alpha"));
        assert!(text.contains("· beta"));
    }
}
