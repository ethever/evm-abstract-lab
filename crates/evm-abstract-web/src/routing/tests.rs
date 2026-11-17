use super::*;

#[test]
fn standalone_paths_and_content_links_normalize_without_analysis_fallback() {
    assert_eq!(Route::parse("/", "?anything=ignored").page, Page::Analysis);
    assert_eq!(
        Route::parse("/decompiler/", "").url().as_deref(),
        Some("/decompiler")
    );
    let hash = "AB".repeat(32);
    let config = "CD".repeat(32);
    let route = Route::parse("/decompiler", &format!("?sha256=0x{hash}&config={config}"));
    assert_eq!(
        route.url().unwrap(),
        format!(
            "/decompiler?sha256={}&config={}",
            hash.to_lowercase(),
            config.to_lowercase()
        )
    );
    for query in [
        "?sha256=no",
        "?sha256=a&sha256=b",
        "?address=a&sha256=b",
        "?config=0",
        "?provider=a",
        "?bogus=1",
        "?sha256=%ZZ",
    ] {
        let route = Route::parse("/decompiler", query);
        assert_eq!(route.page, Page::Decompiler);
        assert!(route.error.is_some(), "{query}");
        assert!(route.source.is_none());
        assert!(route.url().is_none());
    }
}

#[test]
fn address_origin_selectors_roundtrip_and_use_one_parameter_vocabulary() {
    let address = "AB".repeat(20);
    let route = Route::parse(
        "/decompiler",
        &format!("?address=0x{address}&provider=RPC%20A%2FB&chain=001&block=0x2a"),
    );
    assert_eq!(
        route.url().unwrap(),
        format!(
            "/decompiler?address=0x{}&provider=RPC%20A%2FB&block=42&chain=1",
            address.to_lowercase()
        )
    );
    let encoded = route.url().unwrap();
    let (path, query) = encoded.split_once('?').unwrap();
    assert_eq!(Route::parse(path, query), route);
    let hash = "CD".repeat(32);
    assert_eq!(
        parse_block(&format!("0x{hash}")).unwrap(),
        BlockSelector::Hash(format!("0x{}", hash.to_lowercase()))
    );
    assert!(parse_block("18446744073709551616").is_err());
    assert!(parse_chain("0x1").is_err());
    assert!(parse_chain(&"9".repeat(100)).is_err());
    assert!(
        Route::parse(
            "/decompiler",
            &format!("?address=0x{address}&config={hash}")
        )
        .error
        .is_some()
    );
    assert!(
        Route::parse("/decompiler", &format!("?sha256={hash}&chain=1"))
            .error
            .is_some()
    );
}
