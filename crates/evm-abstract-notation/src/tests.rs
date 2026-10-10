use super::{Symbol, WideSymbol, normalize_subscripts, parse_state, subscript};

#[test]
fn identities_share_text_notation_without_collapsing_namespaces() {
    for (symbol, expected) in [
        (Symbol::State(128), "σ₁₂₈"),
        (Symbol::ProjectedState(128), "σᵖ₁₂₈"),
        (Symbol::Program(12), "P₁₂"),
        (Symbol::Block(0), "B₀"),
        (Symbol::Edge(17), "e₁₇"),
        (Symbol::Effect(17), "μ₁₇"),
        (Symbol::Frame(1), "f₁"),
        (Symbol::Code(0), "C₀"),
        (Symbol::Certificate(0), "cert₀"),
    ] {
        assert_eq!(symbol.to_string(), expected);
    }
    assert_eq!(Symbol::State(128).index(), 128);
    assert_ne!(
        Symbol::State(0).to_string(),
        Symbol::ProjectedState(0).to_string()
    );
}

#[test]
fn subscript_supports_all_digits_and_full_width_ids_without_touching_values() {
    assert_eq!(subscript(1023456789), "₁₀₂₃₄₅₆₇₈₉");
    assert_eq!(
        normalize_subscripts(&subscript(usize::MAX)),
        usize::MAX.to_string()
    );
    assert_eq!(
        normalize_subscripts("%128 = φ(σ₁₂₈:%127) · μ₄→μ₁₇"),
        "%128 = φ(σ128:%127) · μ4→μ17"
    );
}

#[test]
fn native_state_search_preserves_id_for_all_supported_spellings() {
    for query in [
        "128",
        "σ₁₂₈",
        "σ128",
        "σ_128",
        "sigma_128",
        "\\sigma_{128}",
        "S128",
        "s128",
        "  σ₁₂₈  ",
    ] {
        assert_eq!(parse_state(query), Some(128), "{query}");
    }
    for query in [
        "",
        "σ",
        "σᵖ₁₂₈",
        "μ₁₂₈",
        "P128",
        "%128",
        "-1",
        "+1",
        "sigma_{128",
        "sigma_128}",
        "128x",
        "1 28",
    ] {
        assert_eq!(parse_state(query), None, "{query}");
    }
    assert_eq!(
        parse_state(&Symbol::State(usize::MAX).to_string()),
        Some(usize::MAX)
    );
    assert_eq!(parse_state(&format!("{}0", usize::MAX)), None);
}

#[test]
fn wide_identity_names_keep_all_64_bits_on_every_target() {
    for (symbol, base) in [
        (WideSymbol::Expression(u64::MAX), "expr"),
        (WideSymbol::Fresh(u64::MAX), "fresh"),
        (WideSymbol::Scope(u64::MAX), "scope"),
        (WideSymbol::Task(u64::MAX), "task"),
    ] {
        assert_eq!(
            normalize_subscripts(&symbol.to_string()),
            format!("{base}{}", u64::MAX)
        );
    }
}
