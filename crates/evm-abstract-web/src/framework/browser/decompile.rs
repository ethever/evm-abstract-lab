//! Typed decompiler transport shares the same-origin fetch, abort deadline and
//! body handling with analysis; artifact types never enter analysis messages.

use crate::{
    JobOperation, TransportError,
    decompile::{Command, Message},
};
use evm_abstract_protocol::{
    DECOMPILE_API_PATH, DECOMPILE_OPEN_PATH, DecompileJobReply, DecompileJobResultReply,
    DecompileOpenReply, DecompileOpenRequest, DecompileSettings, DecompileSourceInput,
};

pub(super) async fn execute(command: Command) -> Message {
    match command {
        Command::Open {
            generation,
            request,
        } => Message::Opened {
            generation,
            result: Box::new(open(*request).await),
        },
        Command::Providers { generation } => Message::Providers {
            generation,
            result: super::rpc_providers().await,
        },
        Command::Submit {
            generation,
            request,
        } => Message::Opened {
            generation,
            result: Box::new(
                open(DecompileOpenRequest {
                    settings: Some(DecompileSettings::from_request(&request)),
                    source: DecompileSourceInput::Bytecode(request.bytecode),
                })
                .await,
            ),
        },
        Command::Poll { generation, id } => Message::Status {
            generation,
            operation: JobOperation::Poll,
            result: Box::new(status("GET", &format!("{DECOMPILE_API_PATH}/{id}"), None).await),
        },
        Command::Cancel { generation, id } => Message::Status {
            generation,
            operation: JobOperation::Cancel,
            result: Box::new(status("DELETE", &format!("{DECOMPILE_API_PATH}/{id}"), None).await),
        },
        Command::Result { generation, id } => Message::Result {
            generation,
            id,
            result: Box::new(result(&format!("{DECOMPILE_API_PATH}/{id}/result")).await),
        },
    }
}

async fn open(request: DecompileOpenRequest) -> Result<DecompileOpenReply, TransportError> {
    let body = serde_json::to_string(&request).map_err(TransportError::Encode)?;
    let (status, ok, body) = super::fetch("POST", DECOMPILE_OPEN_PATH, Some(body)).await?;
    let reply: DecompileOpenReply =
        serde_json::from_slice(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}

async fn status(
    method: &str,
    path: &str,
    body: Option<String>,
) -> Result<DecompileJobReply, TransportError> {
    let (status, ok, body) = super::fetch(method, path, body).await?;
    let reply: DecompileJobReply =
        serde_json::from_slice(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}

async fn result(path: &str) -> Result<DecompileJobResultReply, TransportError> {
    let (status, ok, body) = super::fetch("GET", path, None).await?;
    let reply: DecompileJobResultReply =
        serde_json::from_slice(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}
