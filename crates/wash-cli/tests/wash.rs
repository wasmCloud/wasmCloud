mod common;

use common::{output_to_string, wash};

#[test]
fn integration_help_subcommand_check() {
    let help_output = wash()
        .args(["--help"])
        .output()
        .expect("failed to display help text");
    let output = output_to_string(help_output).unwrap();

    assert!(output.contains("claims"));
    assert!(output.contains("ctl"));
    assert!(output.contains("drain"));
    assert!(output.contains("keys"));
    assert!(output.contains("par"));
}
