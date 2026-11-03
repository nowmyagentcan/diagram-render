use std::process::Command;

#[test]
fn version_names_the_tool() {
    let out = Command::new(env!("CARGO_BIN_EXE_diagram-render"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "diagram-render 0.1.0\n"
    );
}
