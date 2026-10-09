//! Public RPC catalogue; endpoint locations and credentials remain server-owned.

/// Same-origin route listing the configured RPC providers.
pub const RPC_PROVIDERS_PATH: &str = "/api/rpc-providers";

record! {
    /// One selectable provider, without its private connection details.
    pub struct RpcProvider {
        /// Stable server-configured ID submitted with analysis requests.
        pub id: String,
        /// Human-readable display name.
        pub name: String,
    }
}

record! {
    /// Public provider catalogue or a typed transport failure.
    pub struct RpcProvidersReply {
        /// Configured providers in their configured order; empty disables RPC input.
        pub result: Result<Vec<RpcProvider>, crate::ApiError>,
    }
}
