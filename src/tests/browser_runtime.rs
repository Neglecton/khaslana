use super::*;

#[test]
fn cached_runtime_without_installation_remains_unchecked() {
    let dir = tempfile::tempdir().unwrap();
    let info = inspect_cached(dir.path());
    assert!(!info.system_checked);
    assert!(!info.mcp_ready);
    assert!(info.node_path.is_none());
    assert!(info.node_version.is_none());
}

#[test]
fn cached_runtime_reads_installation_without_executing_node() {
    let dir = tempfile::tempdir().unwrap();
    let node = dir.path().join("node.exe");
    // 非可执行文件也能读取安装记录：展示路径不应启动或验证该程序。
    fs::write(&node, b"not an executable").unwrap();
    fs::create_dir_all(cli_path(dir.path()).parent().unwrap()).unwrap();
    fs::write(cli_path(dir.path()), b"unused").unwrap();
    let manifest = RuntimeManifest {
        node_path: node.clone(), node_version: "24.0.0".into(), mcp_version: MCP_VERSION.into(),
    };
    fs::write(runtime_dir(dir.path()).join("install.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let info = inspect_cached(dir.path());
    assert!(info.mcp_ready);
    assert!(!info.system_checked);
    assert_eq!(info.node_path, Some(node));
    assert_eq!(info.node_version.as_deref(), Some("24.0.0"));
}
