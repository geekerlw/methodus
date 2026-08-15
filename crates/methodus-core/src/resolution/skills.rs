//! Skill catalog: Methodus-owned packages only (`~/.methodus/skills/*/SKILL.md`).
//! Does not scan `~/.claude`, `~/.cursor`, or other executor user directories.

use std::fs;
use std::path::{Path, PathBuf};

const META_SKILLS: &[&str] = &[
    "skill-creator",
    "find-skills",
    "update-config",
    "keybindings-help",
    "statusline-setup",
    "methodus",
];

#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    pub name: String,
    pub description: String,
    pub source: String,
    pub path: PathBuf,
}

pub fn scan_skills(methodus_home: &Path) -> Vec<DiscoveredSkill> {
    let mut out = Vec::new();
    collect_skill_dir(&mut out, &methodus_home.join("skills"), "builtin");
    out
}

fn collect_skill_dir(out: &mut Vec<DiscoveredSkill>, dir: &Path, source: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let skill_md = entry.path().join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let Some((name, description)) = parse_skill_md(&skill_md) else {
            continue;
        };
        if META_SKILLS.iter().any(|m| *m == name) {
            continue;
        }
        if out.iter().any(|s| s.name == name) {
            continue;
        }
        out.push(DiscoveredSkill {
            name,
            description,
            source: source.to_string(),
            path: skill_md,
        });
    }
}

fn parse_skill_md(path: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(path).ok()?;
    let fallback_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if fallback_name.is_empty() {
        return None;
    }
    if let Some(fm) = extract_frontmatter(&text) {
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(fm) {
            let name = value
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| fallback_name.clone());
            let description = value
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Some((name, description));
        }
    }
    Some((fallback_name, String::new()))
}

fn extract_frontmatter(text: &str) -> Option<&str> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    if let Some((body, _)) = rest.split_once("\n---") {
        return Some(body);
    }
    rest.split_once("\r\n---").map(|(body, _)| body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_skill(dir: &Path, name: &str, description: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
        )
        .unwrap();
    }

    #[test]
    fn scans_methodus_skills_only() {
        let dir = tempdir().unwrap();
        let methodus = dir.path().join("mh");
        write_skill(&methodus.join("skills"), "tcp-debug", "builtin copy");
        write_skill(
            &dir.path().join("user").join(".claude").join("skills"),
            "tcp-debug",
            "must not be seen",
        );
        write_skill(
            &dir.path()
                .join("user")
                .join(".claude")
                .join("plugins")
                .join("nex")
                .join("skills"),
            "nex-init",
            "must not be seen",
        );
        let catalog = scan_skills(&methodus);
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "tcp-debug");
        assert_eq!(catalog[0].source, "builtin");
        assert!(catalog[0].description.contains("builtin"));
    }

    #[test]
    fn skips_meta_skills() {
        let dir = tempdir().unwrap();
        let methodus = dir.path().join("mh");
        write_skill(&methodus.join("skills"), "skill-creator", "meta");
        write_skill(&methodus.join("skills"), "keep-me", "ok");
        let catalog = scan_skills(&methodus);
        assert!(catalog.iter().any(|s| s.name == "keep-me"));
        assert!(catalog.iter().all(|s| s.name != "skill-creator"));
    }
}
