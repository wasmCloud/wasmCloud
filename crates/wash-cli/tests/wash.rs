mod common;

use common::{output_to_string, wash};

// The purpose of this text is to ensure we don't remove subcommands from the help text without knowing.
#[test]
fn integration_help_subcommand_check() {
    let help_output = wash()
        .args(["--help"])
        .output()
        .expect("failed to display help text");
    let output = output_to_string(help_output).unwrap();

    assert!(output.contains("new"));
    assert!(output.contains("build"));
    assert!(output.contains("dev"));
    assert!(output.contains("inspect"));
    assert!(output.contains("par"));
    assert!(output.contains("up"));
    assert!(output.contains("down"));
    assert!(output.contains("app"));
    assert!(output.contains("spy"));
    assert!(output.contains("ui"));
    assert!(output.contains("get"));
    assert!(output.contains("start"));
    assert!(output.contains("scale"));
    assert!(output.contains("stop"));
    assert!(output.contains("update"));
    assert!(output.contains("link"));
    assert!(output.contains("call"));
    assert!(output.contains("label"));
    assert!(output.contains("config"));
    assert!(output.contains("pull"));
    assert!(output.contains("push"));
    assert!(output.contains("reg"));
    assert!(output.contains("completions"));
    assert!(output.contains("ctx"));
    assert!(output.contains("drain"));
    assert!(output.contains("keys"));
    assert!(output.contains("claims"));
}
