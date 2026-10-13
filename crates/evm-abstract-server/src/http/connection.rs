//! Drive one connection while retaining Hyper's unprocessed header bytes.

use super::REQUEST_TIMEOUT;
use axum::Router;
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use tokio::{net::TcpStream, sync::watch};

pub(super) async fn serve(
    stream: TcpStream,
    app: Router,
    mut stopping: watch::Receiver<()>,
) -> Result<(), hyper::Error> {
    let service = TowerToHyperService::new(app);
    let mut stream = tokio_io_timeout::TimeoutStream::new(stream);
    stream.set_write_timeout(Some(REQUEST_TIMEOUT));
    let mut transport = http1::Builder::new();
    transport
        .timer(TokioTimer::new())
        .header_read_timeout(REQUEST_TIMEOUT)
        .max_buf_size(8192);
    let mut connection = transport.serve_connection(TokioIo::new(Box::pin(stream)), service);
    let result = tokio::select! {
        result = &mut connection => result,
        _ = stopping.changed() => {
            std::pin::Pin::new(&mut connection).graceful_shutdown();
            (&mut connection).await
        },
    };
    match result {
        Err(error) if error.is_timeout() => {
            // Hyper starts its header deadline even before any next-request byte
            // arrives. An empty parser buffer therefore means an unused or idle
            // connection; partial headers remain an error, including bytes read
            // ahead with a previous request or its body. This driver's Hyper
            // timeout is the header deadline: Hyper 1.11.1's other timeout cause
            // is HTTP/2 keepalive, which this HTTP/1 driver does not use. Socket
            // write deadlines are I/O errors and do not match is_timeout().
            if connection.into_parts().read_buf.is_empty() {
                Ok(())
            } else {
                Err(error)
            }
        }
        other => other,
    }
}
