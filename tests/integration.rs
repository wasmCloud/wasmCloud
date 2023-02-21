mod common;
use cmd_lib::run_fun;

#[test]
fn integration_help_subcommand_check() {
    let wash = env!("CARGO_BIN_EXE_wash");
    let output = run_fun!( $wash --help ).expect("failed to display help text");

    println!("output: \n{output}");

    assert!(output.contains("claims"));
    assert!(output.contains("ctl"));
    assert!(output.contains("drain"));
    assert!(output.contains("keys"));
    assert!(output.contains("par"));
}
