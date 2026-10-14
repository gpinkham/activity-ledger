use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub project: String,
    pub size: i64,
    pub mtime: i64,
    /// Set for a sub-agent transcript (`<project>/<session>/subagents/agent-<id>.jsonl`).
    pub subagent: Option<SubagentMeta>,
}

/// What links a sub-agent's transcript to the call that started it, from the
/// `agent-<id>.meta.json` sidecar. Its free-text description is not read.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentMeta {
    pub agent_id: String,
    pub parent_tool_use_id: Option<String>,
    pub agent_type: Option<String>,
}

fn subagent_meta(path: &Path) -> Option<SubagentMeta> {
    let stem = path.file_stem()?.to_str()?;
    let agent_id = stem.strip_prefix("agent-")?.to_string();
    let sidecar = path.with_file_name(format!("{stem}.meta.json"));
    let v: Option<serde_json::Value> = std::fs::read_to_string(sidecar).ok().and_then(|s| serde_json::from_str(&s).ok());
    let field = |k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_str()).map(|s| s.to_string());
    Some(SubagentMeta { agent_id, parent_tool_use_id: field("toolUseId"), agent_type: field("agentType") })
}

/// Walk `root` recursively and return every `*.jsonl` file with its size and
/// mtime. `project` is the name of the file's immediate parent directory, or for a
/// sub-agent transcript, the project folder three levels up.
pub fn scan_dir(root: &Path) -> Vec<ScannedFile> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let parent = path.parent();
        let in_subagents = parent.and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("subagents");
        let project_dir = if in_subagents { parent.and_then(|p| p.parent()).and_then(|p| p.parent()) } else { parent };
        let project = project_dir
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let subagent = if in_subagents { subagent_meta(path) } else { None };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len() as i64;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(ScannedFile { path: path.to_path_buf(), project, size, mtime, subagent });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_subagent_file_belongs_to_its_parents_project_and_carries_its_link() {
        let dir = std::env::temp_dir().join(format!("ci_scan_sub_{}", std::process::id()));
        let sub = dir.join("-Users-me-projA").join("sess-1").join("subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.join("-Users-me-projA").join("sess-1.jsonl"), "{}\n").unwrap();
        fs::write(sub.join("agent-a1.jsonl"), "{}\n").unwrap();
        fs::write(sub.join("agent-a1.meta.json"), r#"{"agentType":"Explore","toolUseId":"toolu_9","description":"secret words"}"#).unwrap();

        let mut found = scan_dir(&dir);
        found.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(found.len(), 2, "the .meta.json sidecar is not a transcript");
        let child = found.iter().find(|f| f.subagent.is_some()).unwrap();
        assert_eq!(child.project, "-Users-me-projA");
        let meta = child.subagent.as_ref().unwrap();
        assert_eq!(meta.agent_id, "a1");
        assert_eq!(meta.parent_tool_use_id.as_deref(), Some("toolu_9"));
        assert_eq!(meta.agent_type.as_deref(), Some("Explore"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_jsonl_files_with_project_names() {
        let dir = std::env::temp_dir().join(format!("ci_scan_{}", std::process::id()));
        let proj = dir.join("-Users-me-projA");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("a.jsonl"), "{}\n").unwrap();
        fs::write(proj.join("notes.txt"), "ignore me").unwrap();

        let found = scan_dir(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project, "-Users-me-projA");
        assert!(found[0].size > 0);

        fs::remove_dir_all(&dir).ok();
    }
}
