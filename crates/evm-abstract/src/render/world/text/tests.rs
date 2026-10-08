use super::{render, write_table};
use crate::{
    Fork,
    analysis::{ExecutionConfig, analyze_world},
    domain::AbstractValue,
    world::{Account, ByteArray, Entry, World},
};
use alloy_primitives::{Address, U256};

#[test]
fn wrapping_keeps_every_character_in_a_long_abstract_value() {
    let value = format!("{{{}}}", "⊤栈值abc0123".repeat(30));
    let mut output = String::new();
    write_table(&mut output, "", &["Value"], &[vec![value.clone()]]);
    let reconstructed = output
        .lines()
        .skip(2)
        .map(str::trim_end)
        .collect::<String>();
    assert_eq!(reconstructed, value);
    assert!(output.lines().all(|line| line.chars().count() <= 72));
}

#[test]
fn wrapping_keeps_full_word_patterns_intact_at_the_old_boundary() {
    for pattern in [
        format!("0x{}", "*".repeat(64)),
        format!("0x{}[000*]**", "0".repeat(61)),
        format!("0x{}", "f".repeat(64)),
    ] {
        let value = format!("{}[{pattern}]{}", "前".repeat(69), ";suffix".repeat(12));
        let mut output = String::new();
        write_table(&mut output, "", &["Value"], &[vec![value.clone()]]);
        assert!(
            output.lines().skip(2).any(|line| line.contains(&pattern)),
            "{output}"
        );
        let reconstructed = output
            .lines()
            .skip(2)
            .map(str::trim_end)
            .collect::<String>();
        assert_eq!(reconstructed, value);
        assert!(output.lines().all(|line| line.chars().count() <= 72));
    }
}

#[test]
fn the_longest_partial_word_pattern_sets_the_column_width() {
    let pattern = format!("0x{}", "[01**]".repeat(64));
    assert_eq!(pattern.len(), 386);
    let value = format!("before:{pattern};after");
    let mut output = String::new();
    write_table(&mut output, "", &["Value"], &[vec![value.clone()]]);
    assert!(
        output.lines().skip(2).any(|line| line == pattern),
        "{output}"
    );
    let reconstructed = output
        .lines()
        .skip(2)
        .map(str::trim_end)
        .collect::<String>();
    assert_eq!(reconstructed, value);
    assert!(
        output
            .lines()
            .all(|line| line.chars().count() <= pattern.len())
    );
}

#[test]
fn labeled_word_patterns_stay_complete_in_real_value_layouts() {
    for payload in [
        format!("0x{}[000*]**", "0".repeat(61)),
        format!("0x{}", "[01**]".repeat(64)),
    ] {
        let labeled = format!("bits={payload}");
        let width = labeled.len();
        assert!(width >= 76);
        let prefix = "u[0x0,0x100] ";
        let suffix = " mod(0x1)=0x0";
        let value = format!("{prefix}{labeled}{suffix}");
        let mut output = String::new();
        write_table(&mut output, "", &["Value"], &[vec![value]]);
        assert_eq!(
            output.lines().skip(2).collect::<Vec<_>>(),
            [
                format!("{prefix:<width$}"),
                labeled,
                format!("{suffix:<width$}"),
            ],
            "{output}"
        );
        assert_eq!(output.lines().nth(1).unwrap().len(), width);
    }
}

#[test]
fn multiple_word_patterns_wrap_between_words_without_losing_characters() {
    let unknown = format!("0x{}", "*".repeat(64));
    let partial = format!("0x{}[000*]**", "0".repeat(61));
    let value = format!("[{unknown};{partial};{unknown}]");
    let mut output = String::new();
    write_table(&mut output, "", &["Value"], &[vec![value.clone()]]);
    let lines = output.lines().skip(2).collect::<Vec<_>>();
    assert_eq!(lines.len(), 3, "{output}");
    assert!(lines[0].contains(&unknown), "{output}");
    assert!(lines[1].contains(&partial), "{output}");
    assert!(lines[2].contains(&unknown), "{output}");
    let reconstructed = lines.iter().map(|line| line.trim_end()).collect::<String>();
    assert_eq!(reconstructed, value);
}

#[test]
fn ordinary_unicode_cells_keep_their_wrap_limit_in_a_wide_word_column() {
    let pattern = format!("0x{}", "[01**]".repeat(64));
    let ordinary = "入口栈摘要abcdef".repeat(20);
    let mut output = String::new();
    write_table(
        &mut output,
        "",
        &["Value"],
        &[vec![pattern.clone()], vec![ordinary.clone()]],
    );
    let mut lines = output.lines().skip(2);
    assert_eq!(lines.next(), Some(pattern.as_str()));
    let ordinary_lines = lines.map(str::trim_end).collect::<Vec<_>>();
    assert!(ordinary_lines.iter().all(|line| line.chars().count() <= 72));
    assert_eq!(ordinary_lines.concat(), ordinary);
}

#[test]
fn incomplete_or_overlong_patterns_keep_ordinary_wrapping() {
    for pattern in [
        format!("0x{}", "[01**]".repeat(65)),
        format!("0x{}[01*]", "f".repeat(63)),
        format!("bits=0x{}", "[01**]".repeat(65)),
        format!("bits=0x{}[01*]", "f".repeat(63)),
    ] {
        let mut output = String::new();
        write_table(&mut output, "", &["Value"], &[vec![pattern.clone()]]);
        let reconstructed = output
            .lines()
            .skip(2)
            .map(str::trim_end)
            .collect::<String>();
        assert_eq!(reconstructed, pattern);
        assert!(output.lines().all(|line| line.chars().count() <= 72));
    }
}

#[test]
fn a_store_with_unobserved_logs_keeps_the_unknown_flag_visible() {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "unknown logs");
    world.insert(address, Account::empty()).unwrap();
    let mut analysis = analyze_world(
        world,
        Entry::concrete(
            address,
            Address::ZERO,
            AbstractValue::constant(U256::ZERO),
            ByteArray::empty(),
        ),
        ExecutionConfig::default(),
    )
    .unwrap();
    analysis.outcomes[0].store.havoc_all();
    let output = render(&analysis);
    assert!(output.contains("logs_unknown=true"));
    assert_eq!(output.matches("logs_unknown=true").count(), 1);
}
