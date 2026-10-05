// edition:2024
// compile-flags: --test

#[test]
fn compiler_test_callback_uses_static_source_code() {
    let captured = 1;
    let action = |value| value + captured;
    assert_eq!(action(2), 3);
}

macro_rules! static_test {
    () => {
        #[test]
        fn local_macro_test() {
            let action = |value: u8| value + 1;
            assert_eq!(action(1), 2);
        }
    };
}

static_test!();
