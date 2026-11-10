//! User-facing meaning of the execution class declared by each publication.
use evm_abstract_protocol::decompile_model::ExecutionDomain;

pub(super) fn label(domain: ExecutionDomain) -> &'static str {
    match domain {
        ExecutionDomain::Generic => "Generic EVM",
        ExecutionDomain::EthereumTransactions => "Ethereum valid transactions",
    }
}

pub(super) fn assumption(domain: ExecutionDomain) -> &'static str {
    match domain {
        ExecutionDomain::Generic => "Applies to unrestricted EVM inputs.",
        ExecutionDomain::EthereumTransactions => {
            "Assumes a valid included Ethereum transaction, including its contract calls."
        }
    }
}

pub(super) fn declaration(domain: ExecutionDomain) -> String {
    format!("Execution scope: {}", label(domain))
}

pub(super) fn source_comment(domain: ExecutionDomain) -> Option<&'static str> {
    match domain {
        ExecutionDomain::Generic => None,
        ExecutionDomain::EthereumTransactions => Some(
            "// Execution scope: valid included Ethereum transactions and their contract calls.",
        ),
    }
}
