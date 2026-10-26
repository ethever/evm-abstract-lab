//! Source aliases and exact context candidates. Listing preferences never select evidence.

mod calls;
mod scoped;

pub(super) use scoped::Action;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Programs,
    Accounts,
}

#[derive(Default)]
pub(super) struct Directory {
    pub(super) open: bool,
    kind: Kind,
    filter: String,
    all_contexts: bool,
    context_namespace: Option<evm_abstract_protocol::AnalysisNamespace>,
    scoped: scoped::Rows,
    calls: calls::Calls,
}

impl Directory {
    /// Display candidates without changing the canonical model or focus.
    pub(super) fn choose_context(&mut self, namespace: evm_abstract_protocol::AnalysisNamespace) {
        self.kind = Kind::Programs;
        self.context_namespace = Some(namespace);
        self.filter.clear();
    }
}

pub(super) fn short(value: &str) -> String {
    if value.len() > 22 && value.is_ascii() {
        format!("{}…{}", &value[..10], &value[value.len() - 8..])
    } else {
        value.into()
    }
}
