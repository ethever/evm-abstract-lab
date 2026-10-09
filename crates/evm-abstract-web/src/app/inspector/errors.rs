//! Field-by-field presentation of the shared error algebra. Messages supplement
//! typed causes; no JSON or Debug dump is needed to recover expected facts.
use super::value;
use egui::Ui;
use evm_abstract_protocol as api;

pub(super) fn fields(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .striped(false)
        .show(ui, body);
}
fn count(ui: &mut Ui, label: &str, number: Option<usize>) {
    if let Some(number) = number {
        value::text(ui, label, number.to_string());
    }
}
fn word_pair(ui: &mut Ui, expected: &str, observed: &str) {
    value::text(ui, "Expected", expected);
    value::text(ui, "Observed", observed);
}

pub(super) fn api(ui: &mut Ui, error: &api::ApiError) {
    ui.label(&error.message);
    fields(ui, |ui| {
        value::text(ui, "Category", format!("{:?}", error.code))
    });
    match &error.details {
        api::ErrorDetails::Validation(detail) => fields(ui, |ui| {
            value::text(ui, "Validation", format!("{:?}", detail.kind));
            if let Some(field) = &detail.field {
                value::text(ui, "Field", field);
            }
            if let Some(value) = &detail.value {
                value::text(ui, "Rejected value", value);
            }
        }),
        api::ErrorDetails::Bytecode(detail) => fields(ui, |ui| bytecode(ui, detail)),
        api::ErrorDetails::Environment(detail) => fields(ui, |ui| environment(ui, detail)),
        api::ErrorDetails::Configuration(detail) => fields(ui, |ui| match detail {
            api::ConfigurationFailure::Environment(detail) => environment(ui, detail),
            api::ConfigurationFailure::Constants => {
                value::text(ui, "Configuration", "Constant capacity must be positive")
            }
            api::ConfigurationFailure::Budget => {
                value::text(ui, "Configuration", "Execution budgets must be positive")
            }
            api::ConfigurationFailure::Facts => {
                value::text(ui, "Configuration", "Fact exchange bounds must be positive")
            }
            api::ConfigurationFailure::Relations => value::text(
                ui,
                "Configuration",
                "Relation and expression bounds must be positive",
            ),
        }),
        api::ErrorDetails::Limit(detail) => fields(ui, |ui| {
            value::text(ui, "Field", &detail.field);
            value::text(ui, "Submitted", detail.value.to_string());
            value::text(
                ui,
                "Allowed range",
                format!("{} … {}", detail.minimum, detail.maximum),
            );
        }),
        api::ErrorDetails::World(detail) => fields(ui, |ui| world(ui, detail)),
        api::ErrorDetails::Rpc(detail) => rpc(ui, detail),
        api::ErrorDetails::Transport(detail) => fields(ui, |ui| {
            value::text(ui, "Transport", format!("{:?}", detail.kind));
            if let Some(status) = detail.status {
                value::text(ui, "HTTP status", status.to_string());
            }
            count(ui, "Byte count / bound", detail.bytes);
        }),
        api::ErrorDetails::Task(detail) => fields(ui, |ui| {
            value::text(ui, "Task failure", format!("{:?}", detail.kind));
            if let Some(id) = detail.id {
                value::text(ui, "Task", id.to_string());
            }
        }),
        api::ErrorDetails::Worker(detail) => fields(ui, |ui| {
            value::text(ui, "Worker failure", format!("{:?}", detail.kind));
            if let Some(code) = detail.os_code {
                value::text(ui, "OS error code", code.to_string());
            }
        }),
        api::ErrorDetails::Ssa(api::SsaFailure::Coverage) => {
            ui.label("Closed SSA requires complete analysis coverage.");
        }
        api::ErrorDetails::Ssa(api::SsaFailure::Invariant(detail)) => fields(ui, |ui| {
            value::text(ui, "SSA contract", format!("{:?}", detail.kind));
            for (label, number) in [
                ("State", detail.state),
                ("Edge", detail.edge),
                ("PC", detail.pc),
                ("Frame", detail.frame),
                ("Stack slot", detail.slot),
                ("Source state", detail.source_state),
                ("Target state", detail.target_state),
                ("Expected", detail.expected),
                ("Observed", detail.observed),
            ] {
                count(ui, label, number);
            }
            if let Some(id) = detail.value {
                value::text(ui, "SSA value", format!("%{id}"));
            }
            if let Some(id) = detail.effect {
                value::text(ui, "SSA effect", format!("μ{id}"));
            }
        }),
    }
}

fn environment(ui: &mut Ui, detail: &api::EnvironmentFailure) {
    match detail {
        api::EnvironmentFailure::TableLimit => value::text(
            ui,
            "Environment",
            "At most 256 block hashes and 4096 blob hashes",
        ),
        api::EnvironmentFailure::BlobIndex(detail) => {
            value::text(
                ui,
                "Environment",
                "Blob index lies outside the declared count",
            );
            value::text(ui, "Index", &detail.index);
            value::text(ui, "Count", &detail.count);
        }
        api::EnvironmentFailure::Destination(detail) => {
            value::text(
                ui,
                "Environment",
                "Logical destination differs from storage owner",
            );
            word_pair(ui, &detail.expected, &detail.observed);
        }
    }
}
fn bytecode(ui: &mut Ui, detail: &api::BytecodeFailure) {
    match detail {
        api::BytecodeFailure::OddHexLength(length) => {
            value::text(ui, "Bytecode", "Odd hexadecimal digit count");
            value::text(ui, "Digits", length.to_string());
        }
        api::BytecodeFailure::InvalidHex(detail) => {
            value::text(ui, "Bytecode", "Invalid hexadecimal digit");
            value::text(ui, "Digit index", detail.index.to_string());
            value::text(ui, "Character", detail.character.to_string());
        }
        api::BytecodeFailure::UnsupportedEof => {
            value::text(ui, "Bytecode", "EOF containers are unsupported")
        }
        api::BytecodeFailure::DelegatedCode(address) => {
            value::text(
                ui,
                "Bytecode",
                "EIP-7702 delegation requires world analysis",
            );
            value::text(ui, "Implementation", address);
        }
        api::BytecodeFailure::InvalidDelegation(detail) => value::text(
            ui,
            "Delegation format",
            match detail {
                api::DelegationFailure::InvalidLength => "Indicator must contain 23 bytes",
                api::DelegationFailure::InvalidMagic => "Invalid marker prefix",
                api::DelegationFailure::UnsupportedVersion => "Unsupported marker version",
            },
        ),
    }
}
fn world(ui: &mut Ui, detail: &api::WorldFailure) {
    match detail {
        api::WorldFailure::MixedFork(detail) => {
            value::text(ui, "World fork", format!("{:?}", detail.world));
            value::text(ui, "Account fork", format!("{:?}", detail.account));
        }
        api::WorldFailure::UnsupportedDelegation(fork) => {
            value::text(ui, "Delegation unavailable", format!("{fork:?}"))
        }
        api::WorldFailure::Conflict(address) => value::text(ui, "Conflicting account", address),
        api::WorldFailure::CodeHash(detail) => {
            value::text(ui, "Code hash mismatch", &detail.address);
            word_pair(ui, &detail.expected, &detail.observed);
        }
        api::WorldFailure::UnknownCodeHash(address) => {
            value::text(ui, "Hash has no observed code", address)
        }
        api::WorldFailure::InvalidAbsence(address) => {
            value::text(ui, "Absence contradicts account facts", address)
        }
    }
}

pub(super) fn rpc(ui: &mut Ui, failure: &api::RpcFailure) {
    ui.label(&failure.message);
    fields(ui, |ui| {
        value::text(ui, "RPC category", format!("{:?}", failure.kind));
        value::text(ui, "Method", &failure.method);
        for (label, observation) in [
            ("Chain", &failure.chain_id),
            ("Block", &failure.block_hash),
            ("Account", &failure.account),
            ("Slot", &failure.slot),
        ] {
            if let Some(observation) = observation {
                value::text(ui, label, observation);
            }
        }
        if let Some(resource) = failure.resource {
            value::text(ui, "Resource", format!("{resource:?}"));
        }
        count(ui, "Limit", failure.limit);
        if let Some(status) = failure.http_status {
            value::text(ui, "HTTP status", status.to_string());
        }
        if let Some(code) = failure.rpc_code {
            value::text(ui, "RPC code", code.to_string());
        }
        count(ui, "JSON line", failure.json_line);
        count(ui, "JSON column", failure.json_column);
        if let Some(cause) = &failure.cause {
            rpc_cause(ui, cause);
        }
    });
}
fn header(value: &api::RpcHeaderValue) -> String {
    match value {
        api::RpcHeaderValue::Absent => "Not reported".into(),
        api::RpcHeaderValue::Quantity(value)
        | api::RpcHeaderValue::Hash(value)
        | api::RpcHeaderValue::Address(value) => value.clone(),
    }
}
fn rpc_cause(ui: &mut Ui, cause: &api::RpcFailureCause) {
    match cause {
        api::RpcFailureCause::Configuration(reason) => value::text(
            ui,
            "Acquisition configuration",
            match reason {
                api::RpcConfigurationReason::Limits => {
                    "Acquisition limits are outside the supported range"
                }
                api::RpcConfigurationReason::InitialAccounts => {
                    "Initial accounts must be unique and within account/slot bounds"
                }
                api::RpcConfigurationReason::StorageAccountMissing => {
                    "Storage owner was not acquired"
                }
            },
        ),
        api::RpcFailureCause::Response(reason) => response(ui, reason),
        api::RpcFailureCause::Code(reason) => bytecode(ui, reason),
        api::RpcFailureCause::World(reason) => world(ui, reason),
        api::RpcFailureCause::ChainMismatch(observed) => {
            value::text(ui, "Observed chain ID", observed)
        }
        api::RpcFailureCause::BlockMismatch(observed) => {
            value::text(ui, "Observed block hash", observed)
        }
        api::RpcFailureCause::Transport(detail) => {
            for (label, present) in [
                ("Timeout", detail.timeout),
                ("Connection", detail.connect),
                ("Request builder", detail.builder),
                ("Request processing", detail.request),
                ("Body transfer", detail.body),
                ("Response decoding", detail.decode),
                ("Redirect", detail.redirect),
            ] {
                if present {
                    value::text(ui, "Transport classification", label);
                }
            }
            value::text(ui, "Native diagnostic", &detail.native_diagnostic);
        }
        api::RpcFailureCause::Runtime(code) => value::text(
            ui,
            "Reactor OS error",
            code.map_or("No OS code reported".into(), |code| code.to_string()),
        ),
    }
}
fn response(ui: &mut Ui, reason: &api::RpcResponseReason) {
    match reason {
        api::RpcResponseReason::BlockNumber(detail) => {
            value::text(ui, "Response", "Block height differs");
            word_pair(ui, &detail.expected, &detail.observed);
        }
        api::RpcResponseReason::BlobHeaderFields(detail) => {
            value::text(ui, "Response", "Blob accounting fields must occur together");
            value::text(
                ui,
                "Excess blob gas reported",
                detail.excess_blob_gas.to_string(),
            );
            value::text(
                ui,
                "Blob gas used reported",
                detail.blob_gas_used.to_string(),
            );
        }
        api::RpcResponseReason::FeeHistory(detail) => {
            value::text(ui, "Response", "Fee history differs from the pinned block");
            word_pair(ui, &detail.expected, &detail.observed);
            value::text(ui, "Returned fee entries", detail.fees.to_string());
        }
        api::RpcResponseReason::Schema => value::text(ui, "Response", "Typed RPC schema mismatch"),
        api::RpcResponseReason::Version => value::text(ui, "Response", "Expected JSON-RPC 2.0"),
        api::RpcResponseReason::Id(detail) => {
            value::text(ui, "Response", "Request ID mismatch");
            value::text(ui, "Expected ID", detail.expected.to_string());
            value::text(
                ui,
                "Observed ID",
                detail
                    .observed
                    .map_or("Missing".into(), |id| id.to_string()),
            );
        }
        api::RpcResponseReason::ResultAndError => {
            value::text(ui, "Response", "Result and error were both supplied")
        }
        api::RpcResponseReason::NullError => value::text(ui, "Response", "Error member was null"),
        api::RpcResponseReason::HeaderChanged(detail) => {
            value::text(ui, "Changed header field", format!("{:?}", detail.field));
            word_pair(ui, &header(&detail.expected), &header(&detail.observed));
        }
        api::RpcResponseReason::MissingBlockNumber => {
            value::text(ui, "Response", "No block height for a canonical check")
        }
        api::RpcResponseReason::StorageLength(detail) => {
            value::text(ui, "Response", "Storage result has an invalid byte width");
            value::text(ui, "Expected bytes", detail.expected.to_string());
            value::text(ui, "Observed bytes", detail.observed.to_string());
        }
    }
}
