use std::{
    fmt::Write,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct ProbeDirectory(PathBuf);

impl ProbeDirectory {
    fn new() -> Self {
        for suffix in 0u64.. {
            let path = std::env::temp_dir().join(format!(
                "analysis-kernel-capability-{}-{suffix}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create capability probe directory: {error}"),
            }
        }
        unreachable!("exhausted capability probe directory names")
    }

    fn check(&self, target: &[&str]) -> Output {
        Command::new(env!("CARGO"))
            .current_dir(&self.0)
            // Cargo can configure these independently. Both must be private so
            // the child never waits for the outer test's artifact locks.
            .env("CARGO_TARGET_DIR", self.0.join("target"))
            .env("CARGO_BUILD_BUILD_DIR", self.0.join("build"))
            .args([
                "check",
                "--offline",
                "--profile",
                "dev",
                "--color",
                "never",
                "--message-format",
                "short",
                "--manifest-path",
            ])
            .arg(self.0.join("Cargo.toml"))
            .args(target)
            .output()
            .expect("run the isolated capability consumer's Cargo check")
    }
}

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        // Also clean up after assertion failures, without masking their panic.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn toml_path(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for character in path.to_str().expect("UTF-8 Cargo source path").chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                write!(quoted, "\\u{:04x}", u32::from(character)).unwrap();
            }
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

#[test]
fn a_missing_declared_capability_is_a_compile_error_not_a_top_fallback() {
    let directory = ProbeDirectory::new();
    let kernel = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = format!(
        r#"[package]
name = "analysis-kernel-capability-probe"
version = "0.0.0"
edition = "2024"
publish = false

[workspace]

[dependencies]
analysis-kernel = {{ path = {} }}

[lib]
path = "present_route.rs"

[[test]]
name = "missing_route"
path = "missing_route.rs"

# Match the repository's development profile for both explicit check targets.
[profile.dev]
opt-level = 0
debug = 0
lto = "off"
codegen-units = 256
incremental = true
debug-assertions = true
overflow-checks = true
"#,
        toml_path(kernel)
    );
    fs::write(directory.0.join("Cargo.toml"), manifest).unwrap();
    for fixture in ["present_route.rs", "missing_route.rs"] {
        fs::copy(
            kernel.join("tests/cooperation/fixtures").join(fixture),
            directory.0.join(fixture),
        )
        .unwrap();
    }

    let present = directory.check(&["--lib"]);
    assert!(
        present.status.success(),
        "the declared provider must compile against the current kernel: {}",
        String::from_utf8_lossy(&present.stderr)
    );

    let missing = directory.check(&["--test", "missing_route"]);
    let stderr = String::from_utf8(missing.stderr).unwrap();
    assert_eq!(
        missing.status.code(),
        Some(101),
        "expected Cargo's compile-error status for the missing provider: {stderr}"
    );
    let errors: Vec<_> = stderr
        .lines()
        .filter(|line| line.contains(": error"))
        .collect();
    assert!(
        errors.len() == 1
            && errors[0].contains("missing_route.rs:")
            && errors[0].contains("error[E0277]:")
            && errors[0].contains("`Nil: analysis_kernel::Select<Here>`"),
        "expected only the missing provider's trait error: {stderr}"
    );
    assert!(
        !stderr.contains("can't find crate"),
        "missing artifact is not capability evidence: {stderr}"
    );
    eprintln!("accepted capability diagnostic: {}", errors[0]);
}
