//! Actual constructor admission and delivery facts, separate from CALL booleans.

choice! {
    /// Scope recorded by the native control relation; absence never implies a primitive.
    pub enum ProcessControlScope {
        /// One EVM action, with separate residual-chain dependencies.
        Primitive,
    }
}
choice! {
    /// Exceptional halt of the creating frame before child admission.
    pub enum ProcessParentException {
        /// State-changing creation attempted during read-only execution.
        Static,
        /// Initcode exceeds the fork's permitted size.
        InitCodeSize,
        /// Opcode or memory expansion gas failure.
        Gas,
    }
}
choice! {
    /// Local rejection returns zero without entering constructor code.
    pub enum ProcessRejectionReason {
        /// EVM invocation depth limit.
        Depth,
        /// Creator nonce is already maximal.
        MaxNonce,
        /// Endowment cannot be funded.
        InsufficientBalance,
        /// Existing code or nonce prevents overwrite.
        Collision,
        /// Insufficient child/initcode gas after nonce increment.
        ChildGas,
    }
}
record! {
    /// One actual zero-result allocation alternative.
    pub struct ProcessRejection {
        /// EVM rejection independently of analysis refusal.
        pub reason: ProcessRejectionReason,
        /// Collision destination class when recorded.
        pub destination: Option<super::ProcessOwner>,
        /// Creator nonce increment survives this rejection.
        pub increments_nonce: bool,
    }
}
record! {
    /// Recorded allocation guards, independently of real constructor subscriptions.
    pub struct ProcessCreationTransfer {
        /// CREATE or CREATE2.
        pub action: super::ProcessAction,
        /// Full catalogue currency.
        pub epoch: super::ProcessDigest,
        /// Actual execution/storage owner that performed allocation.
        pub creator: super::ProcessOwner,
        /// Entered creating-frame exceptional alternatives.
        pub exceptions: Vec<ProcessParentException>,
        /// Local zero-result alternatives.
        pub rejections: Vec<ProcessRejection>,
    }
}
variant! {
    /// CREATE returns an address class or zero; it never returns CALL's literal one.
    pub enum ProcessCreationResult {
        /// Actual deployment address in this owner class.
        Address(super::ProcessOwner),
        /// Rejection, reverting constructor or exceptional creation failure.
        Zero,
    }
}
choice! {
    /// Parent-visible bytes independently of returned deployment code.
    pub enum ProcessReturnedData {
        /// Success and exceptional failure expose empty parent returndata.
        Empty,
        /// Actual constructor revert data is retained.
        ConstructorRevert,
    }
}
choice! {
    /// Actual child Store completion; enclosing rollback remains separate.
    pub enum ProcessStoreDisposition {
        /// Constructor effects and deployment were committed on this guard.
        CommitConstructor,
        /// Restore child checkpoint while retaining creator nonce increment.
        RestorePostNonceCheckpoint,
    }
}
choice! {
    /// Constructor completion classification, independently of analysis refusal.
    pub enum ProcessCompletionReason {
        /// Valid returned runtime was deployed.
        Deployed,
        /// Exceptional child, invalid runtime or deposit failure.
        Exceptional,
        /// Constructor reverted and exposed its revert data.
        Reverted,
    }
}
record! {
    /// Deployed valid-runtime class, without invented captured bytecode.
    pub struct ProcessDeployment {
        /// Deployment owner class.
        pub owner: super::ProcessOwner,
        /// Runtime size bound checked by the native rule.
        pub maximum_runtime_size: usize,
        /// First runtime byte excluded by the native rule.
        pub forbidden_first_byte: u8,
    }
}
record! {
    /// One actually delivered constructor completion alternative.
    pub struct ProcessCompletionCase {
        /// Created address class or zero result.
        pub result: ProcessCreationResult,
        /// Actual parent-visible returndata class.
        pub returndata: ProcessReturnedData,
        /// Actual commit or rollback disposition.
        pub store: ProcessStoreDisposition,
        /// Deployment class only for successful deployment.
        pub deployment: Option<ProcessDeployment>,
        /// Native completion classification.
        pub reason: ProcessCompletionReason,
    }
}
