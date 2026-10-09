//! Configured RPC names and URL values for the browser's provider selector.

/// Same-origin route listing the configured RPC providers.
pub const RPC_PROVIDERS_PATH: &str = "/api/rpc-providers";

record! {
    /// One selectable provider and its configured URL value.
    pub struct RpcProvider {
        /// Stable server-configured ID submitted with analysis requests.
        pub id: String,
        /// Human-readable display name.
        pub name: String,
        /// Configured HTTP(S) URL displayed alongside the name.
        pub endpoint: String,
    }
}

record! {
    /// Public provider catalogue or a typed transport failure.
    pub struct RpcProvidersReply {
        /// Configured providers in their configured order; empty disables RPC input.
        pub result: Result<Vec<RpcProvider>, crate::ApiError>,
    }
}
