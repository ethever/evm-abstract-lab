//! Declared root input provenance is independent of execution or source currency.
record! {
    /// An original root with an established physical destination.
    pub struct PhysicalRootInput {
        /// Actual physical destination supplied by the input.
        pub address: String,
    }
}
record! {
    /// A supplied root template with symbolic logical ADDRESS.
    pub struct NamespaceRootInput {
        /// Input namespace reference; never an observed physical account.
        pub coordinate: String,
    }
}
variant! {
    /// Original input binding, available even if no machine state was admitted.
    pub enum RootInputBinding {
        /// Physical input facts apply at this destination.
        Physical(PhysicalRootInput),
        /// This original input row is a template for the symbolic root.
        Namespace(NamespaceRootInput),
    }
}

impl crate::AnalysisReport {
    /// Original input row at this coordinate is a template, independent of the
    /// physical accounts in transaction Stores. Never use it for a physical lookup.
    pub fn input_namespace(&self) -> Option<&str> {
        match &self.metadata.root_binding {
            RootInputBinding::Namespace(source) => Some(&source.coordinate),
            RootInputBinding::Physical(_) => None,
        }
    }
    /// Source label qualified by original input provenance.
    pub fn code_label<'a>(&'a self, block: &'a crate::CfgBlock) -> &'a str {
        if block.process.is_none()
            && self.input_namespace().is_some()
            && block.code_address.as_deref() == self.input_namespace()
        {
            "Supplied input program"
        } else {
            block.code_label()
        }
    }
    /// State owner label without promoting an input coordinate to an account.
    pub fn storage_label<'a>(&'a self, block: &'a crate::CfgBlock) -> &'a str {
        if block.process.is_none()
            && self.input_namespace().is_some()
            && block.storage_address.as_deref() == self.input_namespace()
        {
            "Root input owner"
        } else {
            block.storage_label()
        }
    }
}
