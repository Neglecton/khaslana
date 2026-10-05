use super::{McpToolDraft, WorkflowToolAccess, discovered_mcp_tools};

#[test]
fn new_mcp_server_enables_discovered_tools_without_manual_selection() {
    let tools = discovered_mcp_tools(vec!["click".into(), "take_snapshot".into()], &[], true);
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().all(|tool| tool.allowed));
    assert!(tools.iter().all(|tool| tool.access == WorkflowToolAccess::Write));
}

#[test]
fn retesting_preserves_explicit_tool_choices_and_read_write_types() {
    let previous = vec![
        McpToolDraft { name: "click".into(), allowed: false, access: WorkflowToolAccess::Write },
        McpToolDraft { name: "take_snapshot".into(), allowed: true, access: WorkflowToolAccess::Read },
    ];
    let tools = discovered_mcp_tools(
        vec!["click".into(), "take_snapshot".into(), "new_tool".into()], &previous, true);
    assert!(!tools[0].allowed);
    assert!(tools[1].allowed);
    assert_eq!(tools[1].access, WorkflowToolAccess::Read);
    assert!(tools[2].allowed);
}

#[test]
fn editing_saved_mcp_server_preserves_existing_whitelist() {
    let previous = vec![McpToolDraft {
        name: "take_snapshot".into(), allowed: true, access: WorkflowToolAccess::Read,
    }];
    let tools = discovered_mcp_tools(vec!["click".into(), "take_snapshot".into()], &previous, false);
    assert!(!tools[0].allowed);
    assert!(tools[1].allowed);
    assert_eq!(tools[1].access, WorkflowToolAccess::Read);
}
