// edition:2024
// no-prefer-dynamic
// compile-flags: --emit=link

#![crate_type = "proc-macro"]
extern crate proc_macro;

#[proc_macro_derive(Deserialize)]
pub fn deserialize(_: proc_macro::TokenStream) -> proc_macro::TokenStream {
    "fn external_serde_name_only(value: &dyn std::fmt::Debug) { let _ = value; }"
        .parse()
        .unwrap()
}
