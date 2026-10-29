use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

fn setting(key: &str) -> Option<i32> {
    fs::read_to_string("scenario")
        .ok()?
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name == key).then(|| value.parse().ok()).flatten()
        })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let name = executable
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open("calls.txt")?,
        "{name}\t{}\tcwd={}\tdebug={}\tincremental={}",
        args.join("\t"),
        std::env::current_dir()?.display(),
        std::env::var("CARGO_PROFILE_TEST_DEBUG")?,
        std::env::var("CARGO_INCREMENTAL")?,
    )?;
    if name == "git" && args.first().map(String::as_str) == Some("rev-parse") {
        println!("test-commit");
    }
    if name == "rustc" {
        println!("test-toolchain");
    }
    if name != "cargo" {
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("clean") {
        std::process::exit(setting("CLEAN_EXIT").unwrap_or(0));
    }
    if args.first().map(String::as_str) == Some("xtask") {
        if setting("BLOCK_TEST_LOG").is_some() {
            fs::create_dir("target/coverage/tests.txt")?;
        }
        std::process::exit(setting("FIXTURE_EXIT").unwrap_or(0));
    }
    if args.iter().any(|arg| arg == "--no-report") {
        println!("test stdout");
        eprintln!("test stderr");
        std::process::exit(setting("TEST_EXIT").unwrap_or(0));
    }
    if args.get(1).map(String::as_str) == Some("report") {
        println!("report stdout");
        eprintln!("report stderr");
        if args.iter().any(|arg| arg == "--fail-under-lines") {
            std::process::exit(setting("THRESHOLD_EXIT").unwrap_or(0));
        }
        if let Some(code) = setting("REPORT_EXIT") {
            std::process::exit(code);
        }
        for pair in args.windows(2) {
            match pair[0].as_str() {
                "--output-path" => fs::write(&pair[1], "report")?,
                "--output-dir" => {
                    let html = Path::new(&pair[1]).join("html");
                    fs::create_dir_all(&html)?;
                    fs::write(html.join("index.html"), "report")?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
