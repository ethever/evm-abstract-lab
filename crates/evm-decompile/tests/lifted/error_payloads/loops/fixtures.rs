use super::super::fixtures::{Asm, DS_MATH, relative};
use evm_decompile::{Input, U256};

#[derive(Clone, Copy, Debug)]
pub(super) enum Bound {
    Upper,
    FalseOr,
    TrueAnd,
    Missing,
    TrueOr,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Spec {
    pub(super) slot: u64,
    pub(super) header: u64,
    pub(super) base: u64,
    pub(super) coefficient: u64,
    pub(super) growth: u64,
    pub(super) counter_step: u64,
    pub(super) limit: u64,
    pub(super) descending: bool,
    pub(super) bound: Bound,
}

impl Spec {
    pub(super) fn iterations(self, n: u64) -> u64 {
        if self.descending {
            n
        } else {
            n.div_ceil(self.counter_step)
        }
    }

    pub(super) fn pointer(self, n: u64) -> u64 {
        self.base + self.coefficient * n + self.growth * self.iterations(n)
    }
}

pub(super) const SPECS: [Spec; 4] = [
    Spec {
        slot: 64,
        header: 128,
        base: 512,
        coefficient: 13,
        growth: 37,
        counter_step: 1,
        limit: 7,
        descending: false,
        bound: Bound::Upper,
    },
    Spec {
        slot: 96,
        header: 160,
        base: 768,
        coefficient: 45,
        growth: 83,
        counter_step: 2,
        limit: 9,
        descending: false,
        bound: Bound::FalseOr,
    },
    Spec {
        slot: 256,
        header: 384,
        base: 1024,
        coefficient: 7,
        growth: 141,
        counter_step: 3,
        limit: 11,
        descending: false,
        bound: Bound::TrueAnd,
    },
    Spec {
        slot: 0,
        header: 192,
        base: 640,
        coefficient: 29,
        growth: 59,
        counter_step: 1,
        limit: 5,
        descending: true,
        bound: Bound::FalseOr,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Change {
    None,
    InitialWrap,
    GrowthWrap,
    SlotAlias,
    HeaderOutput,
    ArbitraryOutput,
    BranchGrowth,
    EntryBypass,
    SideEntry,
    ExtraBackedge,
    NoProgress,
    UpdatedRead,
    StalePointer,
    PreviousIterationPointer,
    NestedClobber,
    EarlyError,
    FeasibleClobber,
}

fn bounds(asm: &mut Asm, spec: Spec) {
    if matches!(spec.bound, Bound::Missing) {
        return;
    }
    // n > limit is invalid. OR-false supplies both negative facts; AND-true
    // supplies both positive facts. The independent flag is not a length fact.
    asm.op(0x80).number(spec.limit).op(0x90).op(0x11);
    match spec.bound {
        Bound::Upper => {
            asm.target("reject").op(0x57);
        }
        Bound::FalseOr => {
            asm.number(64).op(0x35).op(0x17).target("reject").op(0x57);
        }
        Bound::TrueAnd | Bound::TrueOr => {
            asm.op(0x15).number(64).op(0x35).op(0x15);
            asm.op(if matches!(spec.bound, Bound::TrueAnd) {
                0x16
            } else {
                0x17
            })
            .op(0x15)
            .target("reject")
            .op(0x57);
        }
        Bound::Missing => unreachable!(),
    }
}

fn constructor(asm: &mut Asm, slot: u64, change: Change) {
    if !matches!(
        change,
        Change::StalePointer | Change::PreviousIterationPointer
    ) {
        asm.number(slot).op(0x51);
    }
    relative(asm, 0, U256::from(0x08c379a0_u64) << 224);
    relative(asm, 4, U256::from(32));
    relative(asm, 36, U256::from(DS_MATH.len()));
    let mut message = [0; 32];
    message[..DS_MATH.len()].copy_from_slice(DS_MATH);
    relative(asm, 68, U256::from_be_bytes(message));
    if change == Change::UpdatedRead {
        asm.number(slot)
            .op(0x51)
            .number(1)
            .op(0x01)
            .number(slot)
            .op(0x52);
    }
    asm.number(100)
        .op(0x01)
        .number(slot)
        .op(0x51)
        .op(0x80)
        .op(0x91)
        .op(0x03)
        .op(0x90)
        .op(0xfd);
}

pub(super) fn counted(spec: Spec, change: Change) -> Input {
    program(spec, change).finish()
}

pub(super) fn candidate_pair(unsafe_first: bool) -> Input {
    let mut asm = Asm::default();
    asm.number(96).op(0x35).target("second").op(0x57);
    let (first, second) = if unsafe_first {
        (Change::HeaderOutput, Change::None)
    } else {
        (Change::None, Change::HeaderOutput)
    };
    asm.append(program(SPECS[0], first));
    asm.dest("second");
    asm.append(program(SPECS[0], second));
    asm.finish()
}

fn program(spec: Spec, change: Change) -> Asm {
    let mut asm = Asm::default();
    asm.number(0).op(0x35);
    bounds(&mut asm, spec);
    if matches!(change, Change::EntryBypass | Change::SideEntry) {
        asm.number(64).op(0x35).target("bypass").op(0x57);
    }
    asm.op(0x80)
        .number(spec.header)
        .op(0x52)
        .op(0x80)
        .number(spec.coefficient)
        .op(0x02);
    if change == Change::InitialWrap {
        asm.word(U256::MAX - U256::from(31));
    } else {
        asm.number(spec.base);
    }
    asm.op(0x01).number(spec.slot).op(0x52);
    if matches!(
        change,
        Change::StalePointer | Change::PreviousIterationPointer
    ) {
        asm.number(spec.slot).op(0x51).op(0x90);
    }
    if spec.descending {
        asm.op(0x80);
    } else {
        asm.number(0);
    }
    asm.target("head").op(0x56);
    if matches!(change, Change::EntryBypass | Change::SideEntry) {
        asm.dest("bypass")
            .number(0)
            .target(if change == Change::SideEntry {
                "body"
            } else {
                "head"
            })
            .op(0x56);
    }
    asm.dest("head").op(0x80);
    if spec.descending {
        asm.op(0x15).op(0x15);
    } else {
        asm.number(spec.header).op(0x51).op(0x90).op(0x10);
    }
    asm.target("body")
        .op(0x57)
        .target("error")
        .op(0x56)
        .dest("body");
    if change == Change::FeasibleClobber {
        asm.op(0x81)
            .word(U256::MAX)
            .op(0x01)
            .number(spec.limit - 1)
            .op(0x90)
            .op(0x11)
            .target("after_feasible_clobber")
            .op(0x57)
            .number(32)
            .op(0x35)
            .number(spec.slot)
            .op(0x52)
            .dest("after_feasible_clobber");
    }
    if change == Change::EarlyError {
        asm.number(32).op(0x35).target("early_error").op(0x57);
    }
    if change == Change::PreviousIterationPointer {
        // The same MLOAD operation executes in each epoch. Its previous result
        // is carried separately; the new pointer cannot replace that capture.
        asm.number(spec.slot).op(0x51).op(0x92).op(0x50);
    }
    if change == Change::NestedClobber {
        asm.number(0)
            .dest("inner_head")
            .op(0x80)
            .number(2)
            .op(0x90)
            .op(0x10)
            .target("inner_body")
            .op(0x57)
            .op(0x50)
            .target("inner_exit")
            .op(0x56)
            .dest("inner_body")
            .number(32)
            .op(0x35)
            .number(spec.slot)
            .op(0x52)
            .number(1)
            .op(0x01)
            .target("inner_head")
            .op(0x56)
            .dest("inner_exit");
    }
    // Indexed data writes are below the allocation. Keeping the header intact
    // is necessary for the next source iteration's actual counter bound.
    asm.number(0x42)
        .op(0x81)
        .number(32)
        .op(0x02)
        .number(spec.header + 32)
        .op(0x01)
        .op(0x52);
    asm.number(0x123)
        .number(spec.slot)
        .op(0x51)
        .number(7)
        .op(0x01)
        .op(0x52);
    // The intermediate and final writes both use the saved iteration-start F.
    // Summing their increments would invent a different recurrence.
    asm.number(spec.slot)
        .op(0x51)
        .op(0x80)
        .number(spec.growth / 2)
        .op(0x01)
        .number(spec.slot)
        .op(0x52);
    if change == Change::GrowthWrap {
        // At n=1 the first update wraps exactly into the protected cell.
        asm.word(U256::ZERO.wrapping_sub(U256::from(spec.base + spec.coefficient - spec.slot)));
    } else {
        asm.number(spec.growth);
    }
    asm.op(0x01).number(spec.slot).op(0x52);
    if matches!(change, Change::SlotAlias | Change::ExtraBackedge) {
        asm.number(spec.slot).number(spec.slot).op(0x52);
    }
    if change == Change::BranchGrowth {
        asm.number(32)
            .op(0x35)
            .target("clobber")
            .op(0x57)
            .target("after_clobber")
            .op(0x56)
            .dest("clobber")
            .number(spec.slot)
            .number(spec.slot)
            .op(0x52)
            .target("after_clobber")
            .op(0x56)
            .dest("after_clobber");
    }
    asm.number(96);
    match change {
        Change::HeaderOutput => {
            asm.number(spec.header);
        }
        Change::ArbitraryOutput => {
            asm.number(32).op(0x35);
        }
        _ => {
            asm.number(spec.slot).op(0x51);
        }
    }
    asm.number(0).number(0).number(1).number(255).op(0xfa);
    if change == Change::NoProgress {
        // Host call results may stop on any iteration. An initial n bound
        // therefore cannot bound the number of pointer updates without i++.
        asm.op(0x15).target("error").op(0x57);
    } else {
        asm.op(0x50);
        if spec.descending {
            asm.word(U256::MAX).op(0x01);
        } else {
            asm.number(spec.counter_step).op(0x01);
        }
    }
    if change == Change::ExtraBackedge {
        asm.number(32).op(0x35).target("head").op(0x57);
    }
    asm.target("head").op(0x56).dest("error").op(0x50).op(0x50);
    constructor(&mut asm, spec.slot, change);
    if change == Change::EarlyError {
        asm.dest("early_error").op(0x50).op(0x50);
        constructor(&mut asm, spec.slot, Change::None);
    }
    asm.dest("reject").op(0x00);
    asm
}
