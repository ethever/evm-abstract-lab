//! Alloy SDK calls and transport futures, with bounded acquisition policy.
//! JSON-RPC packets and typed Ethereum decoding belong to Alloy; the private
//! reactor is torn down before cancellation is acknowledged.
use super::{
    AcquisitionLimit, ResponseReason, RpcContext, RpcError, RpcInput, context, invalid, wire,
};
use crate::analysis::control::Control;
use alloy::{
    rpc::{
        client::{RpcClient, RpcClientInner},
        json_rpc::{Id, RequestPacket, ResponsePacket, RpcError as SdkError, RpcRecv, RpcSend},
    },
    transports::{TransportError, TransportErrorKind, TransportFut},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tower::Service;
#[derive(Debug)]
struct Shared {
    input: RpcInput,
    control: Control,
    attempts: AtomicUsize,
    failure: Mutex<Option<RpcError>>,
}
impl Shared {
    fn remember(&self, failure: RpcError) -> TransportError {
        *self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(failure);
        TransportErrorKind::custom_str("bounded analysis RPC acquisition failed")
    }
    fn take_failure(&self) -> Option<RpcError> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}
#[derive(Clone, Debug)]
struct BoundedTransport {
    http: reqwest::Client,
    shared: Arc<Shared>,
}
impl Service<RequestPacket> for BoundedTransport {
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, packet: RequestPacket) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            match this.send(packet).await {
                Ok(response) => Ok(response),
                Err(failure) => Err(this.shared.remember(failure)),
            }
        })
    }
}
impl BoundedTransport {
    async fn send(&self, packet: RequestPacket) -> Result<ResponsePacket, RpcError> {
        let request = packet.as_single().expect("acquisition never batches calls");
        let context = request
            .meta()
            .extensions()
            .get::<RpcContext>()
            .expect("acquisition calls attach their domain context")
            .clone();
        if self.shared.control.cancellation().is_cancelled() {
            return Err(RpcError::Cancelled {
                context: Box::new(context),
            });
        }
        if self
            .shared
            .attempts
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < self.shared.input.max_requests).then_some(used.saturating_add(1))
            })
            .is_err()
        {
            return Err(RpcError::AcquisitionLimit {
                context: Box::new(context),
                resource: AcquisitionLimit::Requests,
                limit: self.shared.input.max_requests,
            });
        }
        let mut response = self
            .http
            .post(&self.shared.input.endpoint)
            .timeout(self.shared.input.timeout)
            .json(&packet)
            .send()
            .await
            .map_err(|source| RpcError::Transport {
                context: Box::new(context.clone()),
                source: source.without_url(),
            })?;
        if !response.status().is_success() {
            return Err(RpcError::Http {
                context: Box::new(context),
                status: response.status().as_u16(),
            });
        }
        let limit = self.shared.input.max_response_bytes;
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(RpcError::ResponseLimit {
                context: Box::new(context),
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
                    context: Box::new(context),
                    limit,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        let Id::Number(id) = request.id() else {
            unreachable!("SDK generates numeric IDs")
        };
        wire::validate(&bytes, *id, &context)?;
        serde_json::from_slice(&bytes).map_err(|source| wire::json_error(&context, source))
    }
}
#[derive(Debug)]
pub(super) struct Client {
    rpc: RpcClient,
    runtime: Option<tokio::runtime::Runtime>,
    shared: Arc<Shared>,
}
impl Client {
    pub(super) fn new(input: &RpcInput, control: &Control) -> Result<Self, RpcError> {
        let context = context(input, "client", None, None);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|source| RpcError::Runtime {
                context: Box::new(context.clone()),
                source,
            })?;
        let http = reqwest::Client::builder()
            .timeout(input.timeout)
            .connect_timeout(input.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|source| RpcError::Transport {
                context: Box::new(context),
                source: source.without_url(),
            })?;
        let shared = Arc::new(Shared {
            input: input.clone(),
            control: control.clone(),
            attempts: AtomicUsize::new(0),
            failure: Mutex::new(None),
        });
        let rpc = RpcClient::from_inner(
            RpcClientInner::new(
                BoundedTransport {
                    http,
                    shared: Arc::clone(&shared),
                },
                false,
            )
            .with_id(1),
        );
        Ok(Self {
            rpc,
            runtime: Some(runtime),
            shared,
        })
    }
    pub(super) fn requests(&self) -> usize {
        self.shared.attempts.load(Ordering::Relaxed)
    }
    pub(super) fn request<P: RpcSend, T: RpcRecv>(
        &mut self,
        context: &RpcContext,
        params: P,
    ) -> Result<T, RpcError> {
        if self.shared.control.cancellation().is_cancelled() {
            return Err(RpcError::Cancelled {
                context: Box::new(context.clone()),
            });
        }
        self.shared.take_failure();
        let mut request = self.rpc.request::<P, Option<T>>(context.method, params);
        request
            .request_mut()
            .meta
            .extensions_mut()
            .insert(context.clone());
        let cancellation = self.shared.control.cancellation();
        let cancelled = async {
            while !cancellation.is_cancelled() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let result = self.runtime.as_ref().expect("cancelled sessions start no new calls").block_on(async {
            tokio::select! {
                biased;
                () = cancelled => Err(RpcError::Cancelled { context: Box::new(context.clone()) }),
                result = request => match result {
                    Ok(Some(value)) => Ok(value),
                    Ok(None) => Err(RpcError::MissingResult { context: Box::new(context.clone()) }),
                    Err(error) => Err(self.shared.take_failure().unwrap_or_else(|| sdk_error(context, error))),
                }
            }
        });
        if matches!(result, Err(RpcError::Cancelled { .. })) {
            drop(self.runtime.take());
        }
        result
    }
}
fn sdk_error(context: &RpcContext, error: TransportError) -> RpcError {
    match error {
        SdkError::ErrorResp(error) => RpcError::Remote {
            context: Box::new(context.clone()),
            code: error.code,
            message: error.message.into_owned(),
        },
        SdkError::NullResp => RpcError::MissingResult {
            context: Box::new(context.clone()),
        },
        SdkError::DeserError { err, .. } | SdkError::SerError(err) => {
            wire::json_error(context, err)
        }
        _ => invalid(context, ResponseReason::Schema),
    }
}
