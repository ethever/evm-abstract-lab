//! Display typed session input coordinates without interpreting human messages.

use super::{api, value};
use egui::Ui;

fn code(ui: &mut Ui, identity: &api::SessionCodeIdentity) {
    value::text(ui, "Code rules", format!("{:?}", identity.fork));
    value::text(ui, "Code mode", format!("{:?}", identity.kind));
    value::text(ui, "Code hash", &identity.hash);
}
fn snapshot(ui: &mut Ui, title: &str, source: &api::SessionSnapshotDescription) {
    match source {
        api::SessionSnapshotDescription::Offline(label) => {
            value::text(ui, title, format!("Offline · {label}"));
        }
        api::SessionSnapshotDescription::Chain(source) => {
            value::text(ui, &format!("{title} chain ID"), &source.chain_id);
            value::text(ui, &format!("{title} block hash"), &source.block_hash);
        }
    }
}
fn fact(ui: &mut Ui, location: &api::InitialFactLocation) {
    use api::InitialFactLocation as F;
    let (kind, address, slot) = match location {
        F::Code(address) => ("Code", address, None),
        F::Balance(address) => ("Balance", address, None),
        F::Nonce(address) => ("Nonce", address, None),
        F::Existence(address) => ("Existence", address, None),
        F::Storage(location) => ("Storage", &location.address, Some(&location.slot)),
        F::StorageComplete(address) => ("Complete storage", address, None),
    };
    value::text(ui, "Conflicting observation", kind);
    value::text(ui, "Account", address);
    if let Some(slot) = slot {
        value::text(ui, "Slot", slot);
    }
}
pub(super) fn show(ui: &mut Ui, detail: &api::SessionFailure) {
    use api::SessionFailure as S;
    super::fields(ui, |ui| match detail {
        S::CatalogFork(detail) => {
            value::text(ui, "Session input", "Mixed catalog execution rules");
            value::text(ui, "Catalog rules", format!("{:?}", detail.catalog));
            value::text(ui, "Program rules", format!("{:?}", detail.program));
        }
        S::CatalogHashCollision(identity) => {
            value::text(ui, "Session input", "Catalog hash collision");
            code(ui, identity);
        }
        S::Snapshot(detail) => {
            value::text(ui, "Session input", "Distinct initial snapshots");
            snapshot(ui, "Expected", &detail.expected);
            snapshot(ui, "Observed", &detail.observed);
            value::text(
                ui,
                "Offline labels",
                "Equal labels do not establish snapshot identity",
            );
        }
        S::FactConflict(location) => fact(ui, location),
        S::MissingCode(identity) => {
            value::text(ui, "Session input", "Code missing from catalog");
            code(ui, identity);
        }
        S::InitcodeBinding(identity) => {
            value::text(ui, "Session input", "Initcode bound as installed runtime");
            code(ui, identity);
        }
        S::EntryCode(address) => {
            value::text(ui, "Session input", "Entry code contradicts observation");
            value::text(ui, "Account", address);
        }
        S::Seed => value::text(
            ui,
            "Session input",
            "Seed does not match the empty snapshot/catalog",
        ),
    });
}
