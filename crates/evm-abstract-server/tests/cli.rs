//! Startup help and configuration failures describe the URL-or-file interface.

use std::{ffi::OsStr, process::Command};

#[test]
fn help_explains_url_and_json_file_forms_and_default_provider() {
    let output = Command::new(env!("CARGO_BIN_EXE_evm-abstract-server"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for text in [
        "--rpc-config <URL_OR_FILE>",
        "HTTP(S) URL",
        "JSON providers file path",
        "default",
        "Default RPC",
    ] {
        assert!(help.contains(text), "missing help text: {text}");
    }
}

#[test]
fn invalid_url_errors_are_explicit_and_do_not_disclose_credentials() {
    for endpoint in [
        "https://secret-user:secret-pass@example.com/private#secret",
        "ftp://secret-user:secret-pass@example.com/private",
        "http://",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_evm-abstract-server"))
            .args(["--rpc-config", endpoint])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("invalid RPC URL"), "{error}");
        assert!(error.contains("HTTP(S) URL"), "{error}");
        assert!(!error.contains("secret"));
        assert!(
            output.stdout.is_empty(),
            "failed configuration started HTTP"
        );
    }
}

#[test]
fn missing_file_error_names_the_file_form_and_url_alternative() {
    let path = std::env::temp_dir()
        .join(format!("evm-web-cli-missing-{}", std::process::id()))
        .join("rpc-providers.json");
    let output = Command::new(env!("CARGO_BIN_EXE_evm-abstract-server"))
        .args([OsStr::new("--rpc-config"), path.as_os_str()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("configuration file (NotFound)"), "{error}");
    assert!(
        error.contains("--rpc-config accepts an HTTP(S) URL"),
        "{error}"
    );
    assert!(
        error.contains("existing JSON configuration file"),
        "{error}"
    );
    assert!(
        output.stdout.is_empty(),
        "failed configuration started HTTP"
    );
}
