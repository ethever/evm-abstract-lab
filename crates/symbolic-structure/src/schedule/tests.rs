use super::Postorder;

#[test]
fn streaming_expansion_retains_siblings_and_repeated_edges_in_order() {
    let mut pending = Postorder::new(0);
    assert_eq!(pending.next(), Some((0, false)));
    assert_eq!(
        pending.expand_with(0, |pending| {
            pending.push(1);
            // Each callback push already lives in the scheduler's retained Vec.
            assert_eq!(pending.pending, [(0, true), (1, false)]);
            pending.push(2);
        }),
        2
    );
    assert_eq!(pending.next(), Some((1, false)));
    assert_eq!(
        pending.expand_with(1, |pending| {
            pending.push(3);
            pending.push(3);
        }),
        2
    );
    assert_eq!(
        pending.collect::<Vec<_>>(),
        [(3, false), (3, false), (1, true), (2, false), (0, true)]
    );
}
