use super::Fork;
use serde_json::{Value, json};

#[test]
fn supported_forks_round_trip_the_existing_lowercase_representation() {
    for (fork, name) in [
        (Fork::Cancun, "cancun"),
        (Fork::Prague, "prague"),
        (Fork::Osaka, "osaka"),
    ] {
        let encoded = serde_json::to_value(fork).unwrap();
        assert_eq!(encoded, json!(name));
        assert_eq!(serde_json::from_value::<Fork>(encoded).unwrap(), fork);
    }
}

#[test]
fn malformed_and_unsupported_fork_representations_are_rejected() {
    for input in [
        json!("london"),
        json!("amsterdam"),
        json!("Osaka"),
        json!(""),
        json!(1),
        json!({"osaka": null}),
        json!(["osaka"]),
        Value::Null,
    ] {
        assert!(
            serde_json::from_value::<Fork>(input.clone()).is_err(),
            "{input}"
        );
    }
}
