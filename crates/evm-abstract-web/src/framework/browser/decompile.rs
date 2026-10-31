//! Typed decompiler transport shares the same-origin fetch, abort deadline and
//! body handling with analysis; artifact types never enter analysis messages.

use crate::{
    JobOperation, TransportError,
    decompile::{Command, Message},
};
use evm_abstract_protocol::{DECOMPILE_API_PATH, DecompileJobReply, DecompileJobResultReply};

pub(super) async fn execute(command: Command) -> Message {
    match command {
        Command::Submit {
            generation,
            request,
        } => Message::Status {
            generation,
            operation: JobOperation::Submit,
            result: Box::new(
                async {
                    let body = serde_json::to_string(&request).map_err(TransportError::Encode)?;
                    status("POST", DECOMPILE_API_PATH, Some(body)).await
                }
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

async fn status(
    method: &str,
    path: &str,
    body: Option<String>,
) -> Result<DecompileJobReply, TransportError> {
    let (status, ok, body) = super::fetch(method, path, body).await?;
    let reply: DecompileJobReply =
        serde_json::from_str(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}

async fn result(path: &str) -> Result<DecompileJobResultReply, TransportError> {
    let (status, ok, body) = super::fetch("GET", path, None).await?;
    let reply: DecompileJobResultReply =
        serde_json::from_str(&body).map_err(|cause| TransportError::Decode { status, cause })?;
    if !ok && reply.result.is_ok() {
        return Err(TransportError::Http { status });
    }
    Ok(reply)
}
