use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=LLVM_LIBFUZZER_PATH");
    let archive = PathBuf::from(
        env::var_os("LLVM_LIBFUZZER_PATH")
            .expect("set LLVM_LIBFUZZER_PATH to Clang's libFuzzer archive"),
    );
    assert!(
        archive.is_file(),
        "missing libFuzzer archive: {}",
        archive.display()
    );
    let library = archive.file_name().unwrap().to_str().unwrap();
    let library = library
        .strip_prefix("lib")
        .unwrap()
        .strip_suffix(".a")
        .unwrap();
    println!("cargo:rerun-if-changed={}", archive.display());
    println!(
        "cargo:rustc-link-search=native={}",
        archive.parent().unwrap().display()
    );
    println!("cargo:rustc-link-lib=static={library}");
    let cpp = if env::var("CARGO_CFG_TARGET_OS").unwrap() == "macos" {
        "c++"
    } else {
        "stdc++"
    };
    println!("cargo:rustc-link-lib={cpp}");
}
