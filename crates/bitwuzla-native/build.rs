//! Build the exception-contained C++ adapter against the native development library.

fn main() {
    println!("cargo:rerun-if-changed=native/shim.cpp");
    let library = pkg_config::Config::new()
        .atleast_version("0.9.1")
        .probe("bitwuzla")
        .expect("Bitwuzla 0.9.1 development library is required");
    let mut build = cc::Build::new();
    // Nix enables FORTIFY_SOURCE, which requires native optimization even for
    // Cargo's debug profile. Keep the checks active instead of suppressing them.
    let optimization = std::env::var("OPT_LEVEL").unwrap_or_else(|_| "1".into());
    build
        .cpp(true)
        .std("c++17")
        .opt_level_str(if optimization == "0" {
            "1"
        } else {
            &optimization
        })
        .file("native/shim.cpp");
    for include in library.include_paths {
        build.include(include);
    }
    build.compile("evm_bitwuzla_adapter");
}
