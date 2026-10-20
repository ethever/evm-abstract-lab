use std::{fs, path::PathBuf, process::Command};

#[test]
fn a_missing_declared_capability_is_a_compile_error_not_a_top_fallback() {
    let executable = std::env::current_exe().unwrap();
    let dependencies = executable.parent().unwrap();
    let libraries: Vec<_> = fs::read_dir(dependencies)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("libanalysis_kernel-")
                && path
                    .extension()
                    .is_some_and(|extension| extension == "rlib")
        })
        .collect();
    assert_eq!(
        libraries.len(),
        1,
        "one tested kernel artifact is required: {libraries:?}"
    );
    let directory =
        std::env::temp_dir().join(format!("analysis-kernel-capability-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/cooperation/fixtures/missing_route.rs");
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(compiler)
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("--extern")
        .arg(format!("analysis_kernel={}", libraries[0].display()))
        .arg("-L")
        .arg(format!("dependency={}", dependencies.display()))
        .arg("--out-dir")
        .arg(&directory)
        .arg(source)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !output.status.success(),
        "a context without a provider unexpectedly compiled"
    );
    assert!(
        stderr.contains("Select") && stderr.contains("Nil"),
        "wrong compiler failure: {stderr}"
    );
    assert!(
        !stderr.contains("can't find crate"),
        "missing artifact is not capability evidence: {stderr}"
    );
}
