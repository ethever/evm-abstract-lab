//! Domain-facing calls do not expose Alloy's erased transport futures.
use super::super::{RpcContext, RpcError};
use super::Loader;
use alloy::rpc::json_rpc::{RpcRecv, RpcSend};
impl Loader {
    pub(super) fn call<P: RpcSend, T: RpcRecv>(
        &mut self,
        context: &RpcContext,
        params: P,
    ) -> Result<T, RpcError> {
        let result = self.client.request(context, params);
        self.progress();
        result
    }
}
