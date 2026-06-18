use aionui_common::constants::AIONUI_FILES_MARKER;
use aionui_file::error::FileError;
use aionui_file::traits::IFileService;

use crate::error::ConversationError;

pub struct StagedMessageAttachments {
    pub content: String,
    pub files: Vec<String>,
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// True when `file_path` is already under `workspace` or is workspace-relative.
pub fn is_inside_workspace(file_path: &str, workspace: &str) -> bool {
    let workspace = workspace.trim();
    if workspace.is_empty() {
        return false;
    }
    let is_absolute = file_path.starts_with('/') || file_path.chars().nth(1) == Some(':');
    if !is_absolute {
        return true;
    }
    let normalized_file = normalize_path(file_path);
    let normalized_workspace = normalize_path(workspace).trim_end_matches('/').to_string();
    normalized_file == normalized_workspace
        || normalized_file.starts_with(&format!("{normalized_workspace}/"))
}

pub fn workspace_relative_ref(file_path: &str, workspace: &str) -> String {
    let trimmed = file_path.trim();
    if workspace.trim().is_empty() {
        return trimmed.to_string();
    }
    let is_absolute = trimmed.starts_with('/') || trimmed.chars().nth(1) == Some(':');
    if !is_absolute {
        return trimmed.trim_start_matches("./").trim_start_matches('/').to_string();
    }
    let normalized_file = normalize_path(trimmed);
    let normalized_workspace = normalize_path(workspace).trim_end_matches('/').to_string();
    if normalized_file == normalized_workspace {
        return ".".to_string();
    }
    if let Some(relative) = normalized_file.strip_prefix(&format!("{normalized_workspace}/")) {
        return relative.to_string();
    }
    trimmed.to_string()
}

pub fn workspace_dest_path(file_path: &str, workspace: &str) -> String {
    let file_name = std::path::Path::new(file_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_path);
    let workspace = workspace.trim_end_matches(['/', '\\']);
    format!("{workspace}/{file_name}")
}

pub fn parse_marker_paths(content: &str) -> (String, Vec<String>) {
    let Some(marker_index) = content.find(AIONUI_FILES_MARKER) else {
        return (content.to_string(), Vec::new());
    };
    let text = content[..marker_index].trim_end().to_string();
    let paths = content[marker_index + AIONUI_FILES_MARKER.len()..]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    (text, paths)
}

pub fn build_content_with_marker(text: &str, refs: &[String]) -> String {
    if refs.is_empty() {
        return text.to_string();
    }
    let body = text.trim_end();
    if body.is_empty() {
        return format!("{AIONUI_FILES_MARKER}\n{}", refs.join("\n"));
    }
    format!("{body}\n\n{AIONUI_FILES_MARKER}\n{}", refs.join("\n"))
}

fn collect_unique_paths(content: &str, files: &[String]) -> (String, Vec<String>) {
    let (text, marker_paths) = parse_marker_paths(content);
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();
    for path in files.iter().chain(marker_paths.iter()) {
        let trimmed = path.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        merged.push(trimmed.to_string());
    }
    (text, merged)
}

pub async fn stage_message_attachments(
    file_service: &dyn IFileService,
    workspace: &str,
    content: &str,
    files: &[String],
) -> Result<StagedMessageAttachments, ConversationError> {
    let workspace = workspace.trim();
    if workspace.is_empty() {
        return Ok(StagedMessageAttachments {
            content: content.to_string(),
            files: files.to_vec(),
        });
    }

    let (text, paths) = collect_unique_paths(content, files);
    if paths.is_empty() {
        return Ok(StagedMessageAttachments {
            content: content.to_string(),
            files: Vec::new(),
        });
    }

    let mut outside = Vec::new();
    let mut inside = Vec::new();
    for path in &paths {
        if is_inside_workspace(path, workspace) {
            inside.push(path.clone());
        } else {
            outside.push(path.clone());
        }
    }

    if !outside.is_empty() {
        let result = file_service
            .copy_files_to_workspace(&outside, workspace, None)
            .await
            .map_err(file_error_to_conversation_error)?;

        if !result.failed_files.is_empty() {
            let failed = result.failed_files.join(", ");
            return Err(ConversationError::BadRequest {
                reason: format!("Failed to copy attachments into workspace: {failed}"),
            });
        }

        let copied: std::collections::HashSet<String> = result.copied_files.into_iter().collect();
        for path in &outside {
            if !copied.contains(path) {
                return Err(ConversationError::BadRequest {
                    reason: format!("Failed to copy attachment into workspace: {path}"),
                });
            }
        }
    }

    let staged_refs: Vec<String> = paths
        .iter()
        .map(|path| {
            if is_inside_workspace(path, workspace) {
                workspace_relative_ref(path, workspace)
            } else {
                workspace_relative_ref(&workspace_dest_path(path, workspace), workspace)
            }
        })
        .collect();

    Ok(StagedMessageAttachments {
        content: build_content_with_marker(&text, &staged_refs),
        files: staged_refs,
    })
}

fn file_error_to_conversation_error(error: FileError) -> ConversationError {
    ConversationError::BadRequest {
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_ref_strips_workspace_prefix() {
        let workspace = "/tmp/finclaw-temp-abc";
        assert_eq!(
            workspace_relative_ref(&format!("{workspace}/费用报销单.xls"), workspace),
            "费用报销单.xls"
        );
    }

    #[test]
    fn keeps_external_absolute_paths_when_not_under_workspace() {
        assert!(!is_inside_workspace(
            "/Users/apple/Downloads/report.xls",
            "/tmp/finclaw-temp-abc"
        ));
    }

    #[test]
    fn parse_marker_paths_splits_body_and_files() {
        let (text, paths) = parse_marker_paths("解析一下\n\n[[AION_FILES]]\nreport.xls");
        assert_eq!(text, "解析一下");
        assert_eq!(paths, vec!["report.xls".to_string()]);
    }

    #[test]
    fn build_content_with_marker_round_trips() {
        let content = build_content_with_marker("hello", &["a.xls".into(), "b.pdf".into()]);
        let (text, paths) = parse_marker_paths(&content);
        assert_eq!(text, "hello");
        assert_eq!(paths, vec!["a.xls", "b.pdf"]);
    }
}
