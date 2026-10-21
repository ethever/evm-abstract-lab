//! Production customization acceptance. All custom state is independent of the default numeric carrier.
#[path = "support/native.rs"]
mod native;

#[path = "cooperation/boundaries.rs"]
mod boundaries;
#[path = "cooperation/bridge.rs"]
mod bridge;
#[path = "cooperation/calls.rs"]
mod calls;
#[path = "cooperation/carrier.rs"]
mod carrier;
#[path = "cooperation/fixtures.rs"]
mod fixtures;
#[path = "cooperation/oracle.rs"]
mod oracle;
#[path = "cooperation/policy.rs"]
mod policy;
#[path = "cooperation/product.rs"]
mod product;
#[path = "cooperation/production.rs"]
mod production;

#[path = "cooperation/global.rs"]
mod global;
