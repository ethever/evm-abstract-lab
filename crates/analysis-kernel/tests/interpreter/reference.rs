//! Concrete finite reachability is the oracle only, never the abstract driver.
use super::{
    driver::Snapshot,
    language::{Instruction, MAX, Pc, Program, State, residue},
};
use analysis_kernel::Reachable;
use std::collections::{BTreeSet, VecDeque};

pub fn covers(snapshot: &Snapshot, program: Program, inputs: u8, check_parity: bool) {
    let mut queue: VecDeque<_> = (0..=MAX)
        .filter(|w| inputs & (1 << w) != 0)
        .map(|w| (Pc::Entry, w))
        .collect();
    let mut visited = BTreeSet::new();
    let mut edges = BTreeSet::new();
    while let Some((pc, word)) = queue.pop_front() {
        if !visited.insert((pc, word)) {
            continue;
        }
        let next = match program.at(pc) {
            Instruction::Equal { value, yes, no } => {
                Some((if word == value { yes } else { no }, word))
            }
            Instruction::Less { limit, yes, no } => {
                Some((if word < limit { yes } else { no }, word))
            }
            Instruction::Add { amount, next } => Some((next, word.saturating_add(amount).min(MAX))),
            Instruction::Return => {
                contains(&snapshot.returned, word);
                None
            }
            Instruction::Fault => {
                contains(&snapshot.faulted, word);
                None
            }
        };
        if let Some((target, value)) = next {
            edges.insert((pc, word, target, value));
            queue.push_back((target, value));
        }
    }
    // Exhaust the finite graph (five points * eight words), without a step cap.
    // Check every reachable state AND every concrete successor edge.
    for (pc, word) in visited {
        contains(&snapshot.states[pc as usize], word);
        if check_parity {
            assert_ne!(snapshot.parities[pc as usize] & residue(word), 0);
        }
    }
    for (pc, word, next, value) in edges {
        assert!(
            snapshot.edges.contains(&(pc, next)),
            "missing abstract edge {pc:?} -> {next:?}"
        );
        contains(&snapshot.states[pc as usize], word);
        contains(&snapshot.states[next as usize], value);
    }
}
fn contains(state: &State, word: u8) {
    assert!(
        matches!(state, Reachable::Value(range) if range.contains(word)),
        "abstract state {state:?} lost concrete word {word}"
    );
}
