use std::{cell::Cell, rc::Rc};

use alloy_primitives::{Address, B256};

use super::super::super::{FirstResumeStatus, MergeResumeStatus};
use super::super::{Arrival, PartitionPolicy, QuotedKey, classify, clone_key};
use super::{HeavyKey, KeyIndex, PolicyKey, Probe, carrier::Classifier};
use crate::{
    analysis::machine::{FrameCode, FrameKey, MachineError, MachineKey},
    resource::WorkBudget,
    world::AddressInput,
};

#[test]
fn fixed_metadata_does_not_pay_for_a_vec_clone() {
    let probe = Rc::new(Probe::default());
    let key = HeavyKey::new(vec![7; 16_384], &probe);
    assert!(std::mem::size_of_val(&key) < key.bytes().len());
    let work = key.key_work();
    probe.reset();
    let mut short = WorkBudget::new(work - 1);
    assert_eq!(clone_key(&key, &mut short), Err(MachineError::Work));
    assert_eq!(probe.quotes.get(), 1);
    assert_eq!(probe.clones.get(), 0);
    assert_eq!(short.used(), 0);

    let mut paid = WorkBudget::new(work);
    let copy = clone_key(&key, &mut paid).unwrap();
    assert_eq!(key, copy);
    assert_ne!(key.bytes().as_ptr(), copy.bytes().as_ptr());
    assert_eq!(copy.key_work(), work);
    assert_eq!(probe.clones.get(), 1);
    assert_eq!(probe.cloned_bytes.get(), key.bytes().len());
    assert_eq!(paid.used(), work);

    let refused = HeavyKey::new(vec![7; 16_384], &probe).with_work(usize::MAX);
    probe.reset();
    let mut unlimited = WorkBudget::new(usize::MAX);
    assert_eq!(clone_key(&refused, &mut unlimited), Err(MachineError::Work));
    assert_eq!(probe.quotes.get(), 1);
    assert_eq!(probe.clones.get(), 0);
    assert_eq!(unlimited.used(), 0);

    let mut exhausted = WorkBudget::new(work);
    assert!(!exhausted.charge(work + 1));
    probe.reset();
    assert_eq!(clone_key(&key, &mut exhausted), Err(MachineError::Work));
    assert_eq!(probe.quotes.get(), 0);
}

struct UnquotedClassifier(Cell<usize>);

impl PartitionPolicy for UnquotedClassifier {
    type Class = ();

    fn classify(&self, _: Arrival) -> Self::Class {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn classification_needs_its_own_quote_and_payment() {
    let arrival = Arrival {
        resume_status: Some(true),
    };
    let legacy = UnquotedClassifier(Cell::new(0));
    assert_eq!(
        classify(&legacy, arrival, &mut WorkBudget::new(usize::MAX)),
        Err(MachineError::Work)
    );
    assert_eq!(legacy.0.get(), 0);
    for (work, available) in [(61, 60), (usize::MAX, usize::MAX)] {
        let probe = Rc::new(Probe::default());
        let policy = Classifier {
            work,
            probe: Rc::clone(&probe),
        };
        let mut budget = WorkBudget::new(available);
        assert_eq!(
            classify(&policy, arrival, &mut budget),
            Err(MachineError::Work)
        );
        assert_eq!(probe.quotes.get(), 1);
        assert_eq!(probe.classifications.get(), 0);
        assert_eq!(budget.used(), 0);
    }
    let probe = Rc::new(Probe::default());
    let policy = Classifier {
        work: 61,
        probe: Rc::clone(&probe),
    };
    let mut budget = WorkBudget::new(61);
    assert_eq!(classify(&policy, arrival, &mut budget), Ok(true));
    assert_eq!(probe.classifications.get(), 1);
    assert_eq!(budget.used(), 61);

    let mut first = WorkBudget::new(1);
    assert_eq!(
        classify(&FirstResumeStatus, arrival, &mut first),
        Ok(Some(true))
    );
    assert_eq!(first.used(), 1);
    let mut merged = WorkBudget::new(1);
    assert_eq!(classify(&MergeResumeStatus, arrival, &mut merged), Ok(()));
    assert_eq!(merged.used(), 1);
}

#[test]
fn wrapper_identity_ignores_work_and_keeps_the_stored_quote() {
    let first = QuotedKey::new(vec![1, 2, 3], 128);
    let equal = QuotedKey::new(vec![1, 2, 3], 256);
    assert_eq!(first, equal);
    assert_eq!(first.cmp(&equal), std::cmp::Ordering::Equal);
    let mut index = KeyIndex::new();
    let mut budget = WorkBudget::new(usize::MAX);
    index.insert_with(first, &mut budget, |_| Ok(11)).unwrap();
    assert_eq!(index.get(&equal, &mut budget), Ok(Some(11)));
    assert_eq!(index.keys().next().unwrap().key_work(), 128);
    assert_eq!(index.total_key_work, 128);
    let copy = clone_key(&equal, &mut budget).unwrap();
    assert_eq!(copy.value(), &vec![1, 2, 3]);
    assert_eq!(copy.key_work(), 256);
    assert_eq!(
        index.insert_with(equal, &mut budget, |_| panic!("occupied builder")),
        Err(MachineError::Shape)
    );
    assert_eq!(index.total_key_work, 128);
}

#[test]
fn native_quotes_include_each_frame_history_heap_and_optional_class() {
    let frame = FrameKey {
        process: None,
        code_address: Address::ZERO,
        code_hash: B256::ZERO,
        mode: FrameCode::Runtime,
        address: Address::ZERO,
        address_value: AddressInput::Concrete(Address::ZERO),
        caller: AddressInput::unknown_caller(),
        is_static: false,
        basic_block_index: 0,
        stack_height: 0,
        jump_history: vec![3, 5],
    };
    let mut longer = frame.clone();
    longer.jump_history.resize(4096, 7);
    assert!(longer.key_work() > frame.key_work() + 4096);
    let key = MachineKey {
        frames: vec![frame, longer],
        code_identity: B256::ZERO,
    };
    let frame_work: usize = key.frames.iter().map(PolicyKey::key_work).sum();
    assert!(key.key_work() > frame_work);
    let key_work = key.key_work();
    let tagged = (key, Some(QuotedKey::new(vec![9; 1024], 4096)));
    assert!(tagged.key_work() > key_work + 4096);
    let mut budget = WorkBudget::new(tagged.key_work());
    assert_eq!(clone_key(&tagged, &mut budget).unwrap(), tagged);
    assert_eq!(budget.used(), tagged.key_work());
    assert!(vec![0_u8; 4096].key_work() > Vec::<u8>::new().key_work() + 4096);
    assert!("x".repeat(4096).key_work() > String::new().key_work() + 4096);
    assert!(Box::new(7_usize).key_work() > 7_usize.key_work());
    assert!(((), false, 7_usize).key_work() > 7_usize.key_work());
    let refused = QuotedKey::new((), usize::MAX);
    assert_eq!(Some(refused.clone()).key_work(), usize::MAX);
    assert_eq!((false, refused.clone()).key_work(), usize::MAX);
    assert_eq!((false, (), refused.clone()).key_work(), usize::MAX);
    assert_eq!(Box::new(refused).key_work(), usize::MAX);
}
