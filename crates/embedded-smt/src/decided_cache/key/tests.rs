use super::*;
use crate::{CacheQuery, Context, DecidedCache, FunctionArg, FunctionSort, term::Node};
use std::hash::RandomState;

const RLIMIT: u32 = 0x0403_0201;

fn scope() -> CacheScope {
    CacheScope {
        enabled: true,
        constraints: 7,
        nodes: 128,
        depth: 64,
    }
}

fn term(op: Op, sort: Sort, args: Vec<Term>) -> Term {
    Term(Arc::new(Node { op, sort, args }))
}

fn hex(value: &str) -> Vec<u8> {
    value
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect()
}

fn bytes(base: &[Bool], model: Option<&Bv>, reserve: usize) -> Vec<u8> {
    let mut used = 0;
    let mut cache = DecidedCache::default();
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        Cancellation::default(),
        &mut cache,
        scope(),
    );
    // Only vary table initialization. Both paths use the same production body.
    encode(
        Encoder {
            limits: Limits::default(),
            cancellation: Cancellation::default(),
            out: Vec::new(),
            nodes: HashMap::with_capacity_and_hasher(reserve, RandomState::new()),
            variables: BTreeMap::new(),
            functions: HashMap::with_capacity_and_hasher(reserve, RandomState::new()),
            edges: 0,
            steps: 0,
            budget: &mut query.budget,
        },
        base,
        model,
        Provider::Z3,
        RLIMIT,
        scope(),
    )
    .unwrap()
}

fn header() -> Vec<u8> {
    // Complete f79 version/provider/rlimit/scope/model/assertion-count bytes.
    hex("01 00 01 02 03 04 01
         07 00 00 00 00 00 00 00 80 00 00 00 00 00 00 00 40 00 00 00 00 00 00 00
         01 01 00 00 00 41")
}

#[test]
fn complete_f79_variable_literal_and_model_bytes() {
    let x = Bv(term(Op::Variable("x".into()), Sort::Bv(8), vec![]));
    let literal = term(Op::Literal("2a".into()), Sort::Bv(8), vec![]);
    let base = [Bool(term(Op::Eq, Sort::Bool, vec![x.0.clone(), literal]))];
    let mut expected = header();
    expected.extend(hex(
        "4e 00 00 00 00 01 08 00 00 00 02 00 00 00 00 00 00 00 00
                         4e 01 00 00 00 01 08 00 00 00 01 02 00 00 00 32 61 00 00 00 00
                         4e 02 00 00 00 00 07 02 00 00 00 00 00 00 00 01 00 00 00
                         52 02 00 00 00 4d 01 52 00 00 00 00",
    ));
    assert_eq!(bytes(&base, Some(&x), 0), expected);
}

#[test]
fn complete_f79_wide_literal_and_operator_parameter_bytes() {
    let x = Bv(term(Op::Variable("wide".into()), Sort::Bv(128), vec![]));
    let literal = term(
        Op::Literal("10000000000000001".into()),
        Sort::Bv(128),
        vec![],
    );
    let base = [Bool(term(Op::Eq, Sort::Bool, vec![x.0.clone(), literal]))];
    let extended = term(Op::ZeroExtend(8), Sort::Bv(136), vec![x.0.clone()]);
    let model = Bv(term(Op::Extract(31, 16), Sort::Bv(16), vec![extended]));
    let mut expected = header();
    expected.extend(hex(
        "4e 00 00 00 00 01 80 00 00 00 02 00 00 00 00 00 00 00 00
                         4e 01 00 00 00 01 80 00 00 00 01 11 00 00 00",
    ));
    expected.extend_from_slice(b"10000000000000001");
    expected.extend(hex("00 00 00 00
                         4e 02 00 00 00 00 07 02 00 00 00 00 00 00 00 01 00 00 00
                         52 02 00 00 00 4d 01
                         4e 03 00 00 00 01 88 00 00 00 22 08 00 00 00 01 00 00 00 00 00 00 00
                         4e 04 00 00 00 01 10 00 00 00 23 1f 00 00 00 10 00 00 00 01 00 00 00 03 00 00 00
                         52 04 00 00 00"));
    assert_eq!(bytes(&base, Some(&model), 0), expected);
}

#[test]
fn complete_f79_zero_argument_declaration_identity_bytes() {
    let context = Context::default();
    let f = context.fresh_function("f", &[], 8);
    let g = context.fresh_function("f", &[], 8);
    let x = f.apply(&[]);
    let y = g.apply(&[]);
    let base = [Bool(term(
        Op::Eq,
        Sort::Bool,
        vec![x.0.clone(), y.0.clone()],
    ))];
    let mut expected = header();
    expected.extend(hex("4e 00 00 00 00 01 08 00 00 00 06 00 00 00 00 00 00 00 00 01 08 00 00 00 00 00 00 00
                         4e 01 00 00 00 01 08 00 00 00 06 01 00 00 00 00 00 00 00 01 08 00 00 00 00 00 00 00
                         4e 02 00 00 00 00 07 02 00 00 00 00 00 00 00 01 00 00 00
                         52 02 00 00 00 4d 01 52 00 00 00 00"));
    assert_eq!(bytes(&base, Some(&x), 0), expected);
    let same = f.apply(&[]);
    let shared = [Bool(term(Op::Eq, Sort::Bool, vec![x.0.clone(), same.0]))];
    assert_ne!(bytes(&shared, Some(&x), 0), expected);
}

#[test]
fn full_bytes_ignore_hash_seed_and_growth_but_keep_order_and_aliases() {
    let context = Context::default();
    let array = context.fresh_array("a", 8, 8);
    let model = context.fresh_bv("model", 8);
    let mut base = Vec::new();
    for index in 0..40 {
        let f = context.fresh_function(
            "f",
            &[FunctionSort::Array {
                index_width: 8,
                element_width: 8,
            }],
            8,
        );
        let value = f.apply(&[FunctionArg::Array(&array)]);
        base.push(value.eq(&model));
        base.push(
            array
                .store(&Bv::from_u64(index, 8), &value)
                .select(&Bv::from_u64(index, 8))
                .eq(&model),
        );
    }
    let expected = bytes(&base, Some(&model), 0);
    for reserve in [0, 4, 128, 1024] {
        assert_eq!(bytes(&base, Some(&model), reserve), expected);
    }
    let different = context.fresh_bv("model", 8);
    assert_ne!(bytes(&base, Some(&different), 0), expected);
    base.reverse();
    assert_ne!(bytes(&base, Some(&model), 0), expected);
}

#[test]
fn growth_refusal_is_prepaid_before_reserve_and_success_has_bounded_capacity() {
    let limit = 32;
    let mut table = HashMap::with_capacity(3);
    let before_capacity = table.capacity();
    for pointer in 0..before_capacity {
        table.insert(pointer, u32::try_from(pointer).unwrap());
    }
    let before = table.clone();
    let target = before_capacity.checked_mul(2).unwrap().max(4).min(limit);
    let growth = crate::decided_cache::budget::hash_growth_work(before_capacity, target * 2);
    let mut used = 0;
    let mut cache = DecidedCache::default();
    {
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        )
        .with_allowance(64 + growth - 1);
        assert!(
            pointers::insert(
                &mut table,
                before_capacity,
                99,
                limit,
                &Cancellation::default(),
                &mut query.budget
            )
            .is_err()
        );
    }
    assert_eq!(table, before);
    assert_eq!(table.capacity(), before_capacity);
    assert_eq!(used, 64); // Failed growth neither reserves nor refunds the hash fee.
    {
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        )
        .with_allowance(64 + growth);
        pointers::insert(
            &mut table,
            before_capacity,
            99,
            limit,
            &Cancellation::default(),
            &mut query.budget,
        )
        .unwrap();
    }
    assert_eq!(table.len(), before_capacity + 1);
    assert_eq!(table[&before_capacity], 99);
    assert!(table.capacity() <= 2 * limit);
    assert_eq!(used, 128 + growth);
}

#[test]
fn cancellation_and_entry_or_capacity_overflow_refuse_before_growth() {
    let mut used = 0;
    let mut cache = DecidedCache::default();
    let cancellation = Cancellation::new();
    let mut table = HashMap::with_capacity(3);
    for pointer in 0..table.capacity() {
        table.insert(pointer, 0);
    }
    let capacity = table.capacity();
    cancellation.cancel();
    {
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        );
        assert!(pointers::insert(&mut table, 99, 1, 32, &cancellation, &mut query.budget).is_err());
    }
    assert_eq!(table.capacity(), capacity);
    assert_eq!(used, 0);
    for limit in [0, usize::MAX] {
        let mut empty = HashMap::new();
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        );
        assert!(
            pointers::insert(
                &mut empty,
                0,
                0,
                limit,
                &Cancellation::default(),
                &mut query.budget
            )
            .is_err()
        );
        assert_eq!(empty.capacity(), 0);
    }
    let mut one = HashMap::new();
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        Cancellation::default(),
        &mut cache,
        scope(),
    );
    pointers::insert(
        &mut one,
        0,
        0,
        1,
        &Cancellation::default(),
        &mut query.budget,
    )
    .unwrap();
    assert!(
        pointers::insert(
            &mut one,
            1,
            1,
            1,
            &Cancellation::default(),
            &mut query.budget
        )
        .is_err()
    );
    assert_eq!(one.len(), 1);
    assert!(one.capacity() <= 4);
}

#[test]
fn original_step_bound_refuses_before_node_table_allocation() {
    let value = Bool::from_bool(true);
    let limits = Limits {
        nodes: 1,
        edges: 0,
        ..Limits::default()
    };
    let mut used = 0;
    let mut cache = DecidedCache::default();
    let mut query = CacheQuery::new(
        usize::MAX,
        &mut used,
        Cancellation::default(),
        &mut cache,
        scope(),
    );
    let mut encoder = Encoder {
        limits,
        cancellation: Cancellation::default(),
        out: Vec::new(),
        nodes: HashMap::new(),
        variables: BTreeMap::new(),
        functions: HashMap::new(),
        edges: 0,
        steps: 2,
        budget: &mut query.budget,
    };
    assert!(encoder.node(&value.0).is_err());
    assert_eq!(encoder.nodes.capacity(), 0);
}

#[test]
fn complete_digest_scan_is_prepaid_and_bucket_eq_remains_linear() {
    let key = [0xa5; 128];
    let mut used = 0;
    let mut cache = DecidedCache::default();
    {
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        )
        .with_allowance(273);
        assert!(crate::decided_cache::index::digest(&key, &mut query.budget).is_none());
    }
    assert_eq!(used, 0);
    {
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        )
        .with_allowance(274);
        assert!(crate::decided_cache::index::digest(&key, &mut query.budget).is_some());
    }
    assert_eq!(used, 274); // Two complete 128-byte scans, 8-byte domains and setup.
    assert_eq!(crate::decided_cache::index::equality_work(4, 128), 516);
}

#[test]
fn pointer_table_setup_refuses_before_encoder_construction() {
    let mut used = 0;
    let mut cache = DecidedCache::default();
    {
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        )
        .with_allowance(127);
        assert!(
            super::query(
                &[],
                None,
                Provider::Z3,
                RLIMIT,
                scope(),
                Limits::default(),
                &mut query.budget
            )
            .is_err()
        );
    }
    assert_eq!(used, 0);
}

#[test]
fn complete_small_limit_keys_match_f79_and_beyond_limit_refuses() {
    let leaf = Bool::from_bool(true);
    let unary = Bool(term(Op::Not, Sort::Bool, vec![leaf.0.clone()]));
    let x = term(Op::Variable("x".into()), Sort::Bv(8), vec![]);
    let literal = term(Op::Literal("2a".into()), Sort::Bv(8), vec![]);
    let equality = term(Op::Eq, Sort::Bool, vec![x, literal]);
    let four = Bool(term(Op::Not, Sort::Bool, vec![equality]));
    let cases = [
        (
            1,
            leaf,
            "4e 00 00 00 00 00 00 01 00 00 00 00 52 00 00 00 00 4d 00",
        ),
        (
            2,
            unary,
            "4e 00 00 00 00 00 00 01 00 00 00 00
                    4e 01 00 00 00 00 08 01 00 00 00 00 00 00 00 52 01 00 00 00 4d 00",
        ),
        (
            4,
            four,
            "4e 00 00 00 00 01 08 00 00 00 02 00 00 00 00 00 00 00 00
                   4e 01 00 00 00 01 08 00 00 00 01 02 00 00 00 32 61 00 00 00 00
                   4e 02 00 00 00 00 07 02 00 00 00 00 00 00 00 01 00 00 00
                   4e 03 00 00 00 00 08 01 00 00 00 02 00 00 00 52 03 00 00 00 4d 00",
        ),
    ];
    for (nodes, assertion, body) in cases {
        let mut expected = header();
        expected[31] = 0; // Candidate namespace, no model target.
        expected.extend(hex(body));
        let base = [assertion];
        let mut used = 0;
        let mut cache = DecidedCache::default();
        let mut query = CacheQuery::new(
            usize::MAX,
            &mut used,
            Cancellation::default(),
            &mut cache,
            scope(),
        );
        let limits = Limits {
            nodes,
            ..Limits::default()
        };
        assert_eq!(
            super::query(
                &base,
                None,
                Provider::Z3,
                RLIMIT,
                scope(),
                limits,
                &mut query.budget
            )
            .unwrap(),
            expected
        );
        let smaller = Limits {
            nodes: nodes - 1,
            ..limits
        };
        assert!(
            super::query(
                &base,
                None,
                Provider::Z3,
                RLIMIT,
                scope(),
                smaller,
                &mut query.budget
            )
            .is_err()
        );
    }
}
