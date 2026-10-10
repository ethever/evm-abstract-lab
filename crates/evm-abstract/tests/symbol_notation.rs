//! Human identities use mathematical symbols while machine IDs and SSA values retain their meaning.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{self, Config, ExecutionConfig, WorldAnalysis},
    bytecode::Program,
    domain::AbstractValue,
    render, ssa,
    world::{Account, ByteArray, Entry, World},
};
use evm_abstract_notation::Symbol;

fn world(code: &str) -> WorldAnalysis {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "notation fixture");
    world
        .insert(address, Account::from_hex(code, Fork::Osaka).unwrap())
        .unwrap();
    analysis::analyze_world(
        world,
        Entry::concrete(
            address,
            Address::ZERO,
            AbstractValue::constant(U256::ZERO),
            ByteArray::empty(),
        ),
        ExecutionConfig::default(),
    )
    .unwrap()
}

#[test]
fn projected_and_native_state_names_remain_distinct_without_renaming_values() {
    let analysis =
        analysis::analyze(Program::from_hex("60015f5500").unwrap(), Config::default()).unwrap();
    let ir = ssa::build(&analysis).unwrap();
    let cfg = render::cfg(&analysis);
    let listing = render::ssa(&analysis, &ir);
    assert!(cfg.contains("σᵖ₀ | B₀ @ 0x0000"), "{cfg}");
    assert!(listing.contains("σᵖ₀ | context="), "{listing}");
    assert!(listing.contains("%0 = PUSH1 0x1"), "{listing}");
    assert!(!listing.contains("%₀"));
    let dot = render::dot(&analysis);
    assert!(dot.contains("σᵖ₀ pc=0x0"), "{dot}");
    let partial = ssa::build_partial_world(analysis.execution()).unwrap();
    let mapping = render::partial_ssa(&analysis, &partial);
    assert!(mapping.contains("projected σᵖ₀ → machine σ₀"), "{mapping}");
    assert!(mapping.contains("σ₀\n"), "{mapping}");
}

#[test]
fn world_text_ssa_and_dot_labels_share_symbols_but_dot_and_json_ids_are_stable() {
    let analysis = world("60015f5500");
    let ir = ssa::build_world(&analysis).unwrap();
    let listing = render::world::ssa(&analysis, &ir);
    let first = &ir.blocks()[0];
    assert!(listing.contains("σ₀ | active=f₀"), "{listing}");
    assert!(
        listing.contains(&format!(
            "{} = effect φ",
            Symbol::Effect(first.effect.result)
        )),
        "{listing}"
    );
    for (_, from, to) in &first.effects {
        assert!(
            listing.contains(&format!(
                "{} → {}",
                Symbol::Effect(*from),
                Symbol::Effect(*to)
            )),
            "{listing}"
        );
    }
    let teaching = render::world::explain(&analysis).unwrap();
    assert!(
        teaching.contains("σ₀ | B₀ @ 0x0000 | f₀ active | code=C₀"),
        "{teaching}"
    );
    assert!(teaching.contains("%0 = PUSH1 0x1"), "{teaching}");
    let report = render::world::text(&analysis);
    assert!(report.contains("σ₀") && report.contains("O₀"), "{report}");
    let dot = render::world::dot(&analysis);
    assert!(dot.contains("S0 [label=\"σ₀"), "{dot}");
    assert!(dot.contains("O0 [label=\"O₀"), "{dot}");
    assert!(dot.contains("S0 -> O0"), "{dot}");
    let json = render::world::json(&analysis).unwrap();
    assert_eq!(json["states"][0]["id"], 0);
    assert_eq!(json["outcomes"][0]["state"], analysis.outcomes()[0].state);
}

#[test]
fn multi_digit_subscript_block_titles_keep_instruction_columns_aligned() {
    let mut code = "5b5f50".repeat(12);
    code.push_str("00");
    let program = Program::from_hex(&code).unwrap();
    let listing = render::disassembly(&program);
    let title = "B₁₀ @ 0x001e:";
    let instructions = listing.split_once(title).unwrap().1;
    let first = instructions.lines().find(|line| !line.is_empty()).unwrap();
    let pc_column = title[..title.find("0x").unwrap()].chars().count() + 2;
    assert_eq!(first.len() - first.trim_start().len(), pc_column);
    assert!(first.trim_start().starts_with("001e: JUMPDEST"), "{first}");
}
