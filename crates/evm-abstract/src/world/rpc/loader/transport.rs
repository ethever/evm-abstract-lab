//! Bounded HTTP and concrete JSON-RPC envelopes on the session-owned reactor.
//! Cancellation releases the pending future and synchronously shuts down its
//! driver tasks before returning; blocking platform DNS may finish during that
//! teardown, so no background worker is left after cancellation is acknowledged.

use super::super::{
    AcquisitionLimit, ResponseReason, RpcContext, RpcError, invalid,
    wire::{Method, Params, Reply, Request},
};
use super::Loader;
use serde::de::DeserializeOwned;
use std::time::Duration;

impl Loader {
    pub(super) fn call<T: DeserializeOwned>(
        &mut self,
        context: &RpcContext,
        method: Method,
        params: Params,
    ) -> Result<T, RpcError> {
        if self.control.cancellation().is_cancelled() {
            return Err(RpcError::Cancelled {
                context: Box::new(context.clone()),
            });
        }
        if self.next_id >= self.input.max_requests {
            return Err(RpcError::AcquisitionLimit {
                context: Box::new(context.clone()),
                resource: AcquisitionLimit::Requests,
                limit: self.input.max_requests,
            });
        }
        self.next_id += 1;
        let id = self.next_id as u64;
        let request = Request {
            jsonrpc: "2.0",
            id,
            method,
            params: &params,
        };
        let limit = self.input.max_response_bytes;
        let pending = async {
            let mut response = self
                .client
                .post(&self.input.endpoint)
                .timeout(self.input.timeout)
                .json(&request)
                .send()
                .await
                .map_err(|source| RpcError::Transport {
                    context: Box::new(context.clone()),
                    source: source.without_url(),
                })?;
            if !response.status().is_success() {
                return Err(RpcError::Http {
                    context: Box::new(context.clone()),
                    status: response.status().as_u16(),
                });
            }
            if response
                .content_length()
                .is_some_and(|length| length > limit as u64)
            {
                return Err(RpcError::ResponseLimit {
                    context: Box::new(context.clone()),
                    limit,
                });
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|source| RpcError::Read {
                context: Box::new(context.clone()),
                source: source.without_url(),
            })? {
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(RpcError::ResponseLimit {
                        context: Box::new(context.clone()),
                        limit,
                    });
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        let cancelled = async {
            while !self.control.cancellation().is_cancelled() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let response = self.runtime.as_ref().expect("cancelled sessions start no further requests").block_on(async {
            tokio::select! {
                biased;
                () = cancelled => Err(RpcError::Cancelled { context: Box::new(context.clone()) }),
                response = pending => response,
            }
        });
        if matches!(response, Err(RpcError::Cancelled { .. })) {
            // Reqwest owns connection-driver tasks as well as the request future.
            // Shut down this cancelled session's reactor so those sockets are
            // released before the caller can acknowledge worker cleanup.
            drop(self.runtime.take());
        }
        self.progress();
        let bytes = response?;
        let reply: Reply<T> = serde_json::from_slice(&bytes).map_err(|source| {
            if source.is_data() {
                RpcError::Response {
                    context: Box::new(context.clone()),
                    reason: ResponseReason::Schema,
                    source: Some(source),
                }
            } else {
                RpcError::Json {
                    context: Box::new(context.clone()),
                    source,
                }
            }
        })?;
        if reply.version.as_deref() != Some("2.0") {
            return Err(invalid(context, ResponseReason::Version));
        }
        if reply.id != Some(id) {
            return Err(invalid(
                context,
                ResponseReason::Id {
                    expected: id,
                    observed: reply.id,
                },
            ));
        }
        if let Some(error) = reply.error {
            if reply.result.is_some() {
                return Err(invalid(context, ResponseReason::ResultAndError));
            }
            let error = error.ok_or_else(|| invalid(context, ResponseReason::NullError))?;
            return Err(RpcError::Remote {
                context: Box::new(context.clone()),
                code: error.code,
                message: error.message,
            });
        }
        reply
            .result
            .flatten()
            .ok_or_else(|| RpcError::MissingResult {
                context: Box::new(context.clone()),
            })
    }
}
