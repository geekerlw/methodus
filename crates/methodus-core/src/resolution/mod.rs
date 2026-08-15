//! Rule-based Face / Method / Skill resolver (no LLM).

mod skills;

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::workspace::is_safe_segment;

pub use skills::{scan_skills, DiscoveredSkill};

const DEFAULT_FACE_ID: &str = "general";
const DEFAULT_METHOD_ID: &str = "general-software";
const LOW_CONFIDENCE: f32 = 0.6;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "to", "for", "of", "in", "on", "and", "or", "with", "from", "by", "at", "is",
    "it", "this", "that", "as", "be", "we", "you", "our", "your", "into", "about", "using", "use",
    "via", "please",
];

#[derive(Debug, Clone)]
pub struct ResolveOpts<'a> {
    pub methodus_home: &'a Path,
    pub request: &'a str,
    pub requested_face: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedMethod {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub recommended_skills: Vec<String>,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedSkill {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolution {
    #[serde(rename = "face")]
    pub face_id: String,
    pub face_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub method: Option<SelectedMethod>,
    #[serde(default)]
    pub skills: Vec<SelectedSkill>,
    pub rationale: String,
    pub confidence: f32,
    #[serde(default)]
    pub low_confidence: bool,
}

impl Resolution {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn parse_json(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    pub fn to_context_markdown(&self, task_title: &str, request: &str) -> String {
        let method_block = match &self.method {
            Some(m) => {
                let steps = m
                    .steps
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "## Method: {name} (`{id}`)\n\n{steps}\n",
                    name = m.name,
                    id = m.id,
                    steps = if steps.is_empty() {
                        "- (no steps listed)".to_string()
                    } else {
                        steps
                    }
                )
            }
            None => "## Method\n\n(none selected)\n".to_string(),
        };
        let skills_block = if self.skills.is_empty() {
            "(none selected)".to_string()
        } else {
            self.skills
                .iter()
                .map(|s| {
                    format!(
                        "- `{}` ({}) — {}\n  path: {}",
                        s.name, s.source, s.description, s.path
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "# Selected context\n\n\
             ## Face: {name} (`{id}`)\n\n\
             {desc}\n\n\
             Rationale: {rationale} (confidence {conf:.2}{low})\n\n\
             {method}\n\
             ## Skills\n\n\
             {skills}\n\n\
             ## Task: {title}\n\n\
             {request}\n\n\
             ## Constraints\n\n\
             - Work inside this workspace unless a project path is explicitly in the request.\n\
             - Do not modify Methodus home state (`~/.methodus`).\n",
            name = self.face_name,
            id = self.face_id,
            desc = self.description,
            rationale = self.rationale,
            conf = self.confidence,
            low = if self.low_confidence {
                "; LOW CONFIDENCE — pin with --face if this is wrong"
            } else {
                ""
            },
            method = method_block,
            skills = skills_block,
            title = task_title,
            request = request,
        )
    }
}

#[derive(Debug, Deserialize)]
struct FaceFile {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    intent_tags: Vec<String>,
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct MethodFile {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    intent_tags: Vec<String>,
    #[serde(default)]
    steps: Vec<String>,
    #[serde(default)]
    recommended_skills: Vec<String>,
    #[serde(skip)]
    path: String,
}

/// Resolve a single Face + Method + matching Skills from files (no LLM).
pub fn resolve(opts: ResolveOpts<'_>) -> Result<Resolution, CoreError> {
    let requested = opts.requested_face.map(str::trim).filter(|s| !s.is_empty());
    if let Some(id) = requested {
        if !is_safe_segment(id) {
            return Err(CoreError::InvalidTaskId(id.to_string()));
        }
    }

    let mut faces = load_faces(opts.methodus_home);
    if faces.is_empty() {
        faces.push(builtin_face());
    }

    let tokens = tokenize(opts.request);
    let (face, face_conf, face_why) = pick_face(&faces, requested, &tokens)?;
    let methods = load_methods(opts.methodus_home);
    let method = pick_method(&methods, &face, &tokens);
    let catalog = scan_skills(opts.methodus_home);
    let selected_skills = pick_skills(&catalog, method.as_ref(), &face, &tokens);

    let mut rationale_parts = vec![face_why];
    if let Some(m) = &method {
        rationale_parts.push(format!("method `{}`", m.id));
    }
    if !selected_skills.is_empty() {
        let names = selected_skills
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        rationale_parts.push(format!("skills [{names}]"));
    }
    let rationale = rationale_parts.join("; ");
    let low_confidence = requested.is_none() && face_conf < LOW_CONFIDENCE;

    Ok(Resolution {
        face_id: face.id,
        face_name: face.name,
        description: face.description,
        method: method.map(|m| SelectedMethod {
            id: m.id.clone(),
            name: if m.name.is_empty() { m.id } else { m.name },
            version: m.version,
            steps: m.steps,
            recommended_skills: m.recommended_skills,
            path: m.path,
        }),
        skills: selected_skills,
        rationale,
        confidence: face_conf,
        low_confidence,
    })
}

/// Pick a single Face. `--face` wins; otherwise tag-match or the `general` seed.
pub fn resolve_face(home: &Path, requested: Option<&str>) -> Result<Resolution, CoreError> {
    resolve(ResolveOpts {
        methodus_home: home,
        request: "",
        requested_face: requested,
    })
}

fn builtin_face() -> FaceFile {
    FaceFile {
        id: DEFAULT_FACE_ID.to_string(),
        name: "General".to_string(),
        description: "Default face used when no domain expert is specified.".to_string(),
        intent_tags: vec!["general".to_string(), "default".to_string()],
        methods: vec![DEFAULT_METHOD_ID.to_string()],
        skills: vec!["workspace-hygiene".to_string()],
    }
}

fn load_faces(home: &Path) -> Vec<FaceFile> {
    let dir = home.join("faces");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("face.yaml");
        if !path.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(face) = serde_yaml::from_str::<FaceFile>(&raw) else {
            continue;
        };
        if is_safe_segment(&face.id) {
            out.push(face);
        }
    }
    out
}

fn load_methods(home: &Path) -> Vec<MethodFile> {
    let dir = home.join("methods");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "yaml" && ext != "yml" {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut method) = serde_yaml::from_str::<MethodFile>(&raw) else {
            continue;
        };
        if is_safe_segment(&method.id) {
            method.path = path.to_string_lossy().into_owned();
            out.push(method);
        }
    }
    out
}

fn pick_face(
    faces: &[FaceFile],
    requested: Option<&str>,
    tokens: &HashSet<String>,
) -> Result<(FaceFile, f32, String), CoreError> {
    if let Some(id) = requested {
        if let Some(face) = faces.iter().find(|f| f.id == id) {
            return Ok((
                clone_face(face),
                1.0,
                format!("user-specified --face (tags: {:?})", face.intent_tags),
            ));
        }
        if id == DEFAULT_FACE_ID {
            let face = builtin_face();
            return Ok((
                face,
                1.0,
                "user-specified --face general (built-in fallback)".to_string(),
            ));
        }
        return Err(CoreError::FaceNotFound(id.to_string()));
    }

    let mut best: Option<(&FaceFile, usize)> = None;
    for face in faces {
        let score = tag_overlap(&face.intent_tags, tokens);
        if score == 0 {
            continue;
        }
        match best {
            Some((_, best_score)) if score <= best_score => {}
            _ => best = Some((face, score)),
        }
    }
    if let Some((face, score)) = best {
        let conf = (0.55 + 0.15 * score as f32).min(0.9);
        return Ok((
            clone_face(face),
            conf,
            format!("intent_tags overlapped request (score {score})"),
        ));
    }

    let face = faces
        .iter()
        .find(|f| f.id == DEFAULT_FACE_ID)
        .map(clone_face)
        .unwrap_or_else(builtin_face);
    Ok((
        face,
        0.45,
        "no intent_tag match; fell back to general".to_string(),
    ))
}

fn pick_method(
    methods: &[MethodFile],
    face: &FaceFile,
    tokens: &HashSet<String>,
) -> Option<MethodFile> {
    if methods.is_empty() {
        return None;
    }
    let preferred: Vec<&MethodFile> = face
        .methods
        .iter()
        .filter_map(|id| methods.iter().find(|m| m.id == *id))
        .collect();
    let pool: Vec<&MethodFile> = if preferred.is_empty() {
        methods.iter().collect()
    } else {
        preferred
    };

    let mut best: Option<(&MethodFile, usize)> = None;
    for method in &pool {
        let score = tag_overlap(&method.intent_tags, tokens);
        match best {
            Some((_, best_score)) if score <= best_score => {}
            _ => best = Some((method, score)),
        }
    }
    if let Some((method, score)) = best {
        if score > 0 {
            return Some(method.clone());
        }
    }
    pool.iter()
        .find(|m| m.id == DEFAULT_METHOD_ID)
        .copied()
        .or_else(|| pool.first().copied())
        .cloned()
}

fn pick_skills(
    catalog: &[DiscoveredSkill],
    method: Option<&MethodFile>,
    face: &FaceFile,
    tokens: &HashSet<String>,
) -> Vec<SelectedSkill> {
    let mut names: Vec<String> = Vec::new();
    for id in &face.skills {
        push_unique(&mut names, id);
    }
    if let Some(m) = method {
        for id in &m.recommended_skills {
            push_unique(&mut names, id);
        }
    }
    for skill in catalog {
        let name_tokens = tokenize(&skill.name.replace('-', " "));
        if name_tokens.iter().any(|t| tokens.contains(t)) {
            push_unique(&mut names, &skill.name);
        }
    }

    let mut selected = Vec::new();
    for name in names {
        if let Some(found) = catalog.iter().find(|s| s.name == name) {
            selected.push(SelectedSkill {
                name: found.name.clone(),
                description: found.description.clone(),
                source: found.source.clone(),
                path: found.path.to_string_lossy().into_owned(),
            });
        }
        if selected.len() >= 8 {
            break;
        }
    }
    selected
}

fn push_unique(names: &mut Vec<String>, id: &str) {
    if !names.iter().any(|n| n == id) {
        names.push(id.to_string());
    }
}

fn clone_face(face: &FaceFile) -> FaceFile {
    FaceFile {
        id: face.id.clone(),
        name: face.name.clone(),
        description: face.description.clone(),
        intent_tags: face.intent_tags.clone(),
        methods: face.methods.clone(),
        skills: face.skills.clone(),
    }
}

fn tag_overlap(tags: &[String], tokens: &HashSet<String>) -> usize {
    tags.iter()
        .filter(|t| tokens.contains(&t.to_ascii_lowercase()))
        .count()
}

fn tokenize(text: &str) -> HashSet<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 2)
        .filter(|w| !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_falls_back_to_builtin() {
        let dir = tempdir().unwrap();
        let r = resolve_face(dir.path(), None).unwrap();
        assert_eq!(r.face_id, "general");
        assert!(r.confidence > 0.0);
        assert!(r.low_confidence);
    }

    #[test]
    fn reads_seed_yaml() {
        let dir = tempdir().unwrap();
        let face_dir = dir.path().join("faces/network");
        fs::create_dir_all(&face_dir).unwrap();
        fs::write(
            face_dir.join("face.yaml"),
            "id: network\nname: Network\ndescription: Packets.\nintent_tags:\n  - net\n",
        )
        .unwrap();
        let r = resolve_face(dir.path(), Some("network")).unwrap();
        assert_eq!(r.face_name, "Network");
        assert_eq!(r.face_id, "network");
        assert!(!r.low_confidence);
    }

    #[test]
    fn missing_requested_face_errors() {
        let dir = tempdir().unwrap();
        let err = resolve_face(dir.path(), Some("nope")).unwrap_err();
        assert!(matches!(err, CoreError::FaceNotFound(_)));
    }

    #[test]
    fn tag_match_picks_network_face_and_method() {
        let dir = tempdir().unwrap();
        let face_dir = dir.path().join("faces/network");
        fs::create_dir_all(&face_dir).unwrap();
        fs::write(
            face_dir.join("face.yaml"),
            "id: network\nname: Network\nintent_tags:\n  - tcp\n  - latency\nmethods:\n  - tcp-latency-investigation\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("methods")).unwrap();
        fs::write(
            dir.path().join("methods/tcp.yaml"),
            "id: tcp-latency-investigation\nname: TCP latency\nintent_tags: [tcp, latency]\nsteps:\n  - collect evidence\nrecommended_skills: [tcp-debug]\n",
        )
        .unwrap();
        let skill_dir = dir.path().join("skills/tcp-debug");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: tcp-debug\ndescription: Debug TCP latency.\n---\n",
        )
        .unwrap();

        let r = resolve(ResolveOpts {
            methodus_home: dir.path(),
            request: "investigate tcp latency on the edge router",
            requested_face: None,
        })
        .unwrap();
        assert_eq!(r.face_id, "network");
        assert!(r.confidence >= 0.6);
        assert!(!r.low_confidence);
        assert_eq!(
            r.method.as_ref().map(|m| m.id.as_str()),
            Some("tcp-latency-investigation")
        );
        assert!(r.skills.iter().any(|s| s.name == "tcp-debug"));
    }

    #[test]
    fn parse_json_roundtrip() {
        let r = Resolution {
            face_id: "general".into(),
            face_name: "General".into(),
            description: "d".into(),
            method: None,
            skills: vec![],
            rationale: "x".into(),
            confidence: 0.45,
            low_confidence: true,
        };
        let parsed = Resolution::parse_json(&r.to_json()).unwrap();
        assert_eq!(parsed.face_id, "general");
        assert!(parsed.low_confidence);
    }
}
