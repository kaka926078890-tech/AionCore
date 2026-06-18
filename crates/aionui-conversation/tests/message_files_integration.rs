use std::fs;
use std::sync::Arc;

use aionui_api_types::WebSocketMessage;
use aionui_conversation::message_files::{parse_marker_paths, stage_message_attachments};
use aionui_file::FileService;
use aionui_realtime::EventBroadcaster;

struct NoopBroadcaster;

impl EventBroadcaster for NoopBroadcaster {
    fn broadcast(&self, _event: WebSocketMessage<serde_json::Value>) {}
}

#[tokio::test]
async fn stage_message_attachments_copies_external_paths_into_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("Downloads");
    let workspace = dir.path().join("finclaw-temp-abc");
    fs::create_dir_all(&downloads).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    fs::write(downloads.join("费用报销单.xls"), b"xls").unwrap();

    let external = downloads.join("费用报销单.xls").to_string_lossy().into_owned();
    let workspace_str = workspace.to_str().unwrap();
    let content = format!("解析一下\n\n[[AION_FILES]]\n{external}");

    let file_service = FileService::new(Arc::new(NoopBroadcaster), vec![dir.path().to_path_buf()]);
    let staged = stage_message_attachments(
        &file_service,
        workspace_str,
        &content,
        &[],
    )
    .await
    .unwrap();

    let (text, paths) = parse_marker_paths(&staged.content);
    assert_eq!(text, "解析一下");
    assert_eq!(paths, vec!["费用报销单.xls"]);
    assert!(workspace.join("费用报销单.xls").is_file());
    assert_eq!(staged.files, vec!["费用报销单.xls".to_string()]);
}
