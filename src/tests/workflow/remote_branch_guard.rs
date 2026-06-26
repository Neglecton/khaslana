use super::*;

#[test]
fn rejects_remote_prefixed_branch_names() {
    let err = validate_remote_branch_name("origin", "origin/feature")
        .unwrap_err()
        .to_string();

    assert!(err.contains("不要带远端名前缀"));
}

#[test]
fn accepts_nested_branch_without_remote_prefix() {
    validate_remote_branch_name("origin", "feature/demo").unwrap();
}
