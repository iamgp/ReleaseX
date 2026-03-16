use std::{fs, path::Path};

#[test]
fn action_exposes_command_input_and_downloads_binary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let action = fs::read_to_string(root.join("action.yml")).expect("read action.yml");

    assert!(action.contains("command:"), "{action}");
    assert!(action.contains("release pr"), "{action}");
    assert!(action.contains("release tag"), "{action}");
    assert!(action.contains("releases/latest/download"), "{action}");
    assert!(action.contains("github.action_repository"), "{action}");
    assert!(action.contains("pyrls ${{ inputs.command }}"), "{action}");
}
