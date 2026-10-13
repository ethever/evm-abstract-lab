//! Graceful shutdown closes idle sockets and drains responses already in flight.

use axum::{
    Router,
    body::{Body, Bytes},
    http::header::CONTENT_LENGTH,
    response::Response,
    routing::get,
};
use evm_abstract_server::http::serve_with_shutdown;
use hyper::body::Frame;
use std::{
    convert::Infallible,
    future::{Future, poll_fn},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    time::timeout,
};

// Shorter than the server's ten-second header deadline: shutdown must close an
// accepted idle connection itself, rather than waiting for its normal expiry.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

async fn read_head(client: &mut BufReader<TcpStream>) -> String {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        assert_ne!(client.read_line(&mut line).await.unwrap(), 0);
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    head
}

#[test]
fn shutdown_closes_an_accepted_idle_keepalive_connection() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            timeout(TEST_TIMEOUT, async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let app = Router::new().route("/", get(|| async { "ready" }));
                let (stop, stopped) = oneshot::channel();
                let server = serve_with_shutdown(listener, app, async {
                    stopped.await.unwrap();
                });
                tokio::pin!(server);

                let (ready, received) = oneshot::channel();
                let client = tokio::spawn(async move {
                    let mut socket = TcpStream::connect(address).await.unwrap();
                    socket
                        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                        .await
                        .unwrap();
                    let mut client = BufReader::new(socket);
                    let head = read_head(&mut client).await;
                    assert!(!head.to_ascii_lowercase().contains("connection: close"));
                    let mut body = [0; 5];
                    client.read_exact(&mut body).await.unwrap();
                    assert_eq!(&body, b"ready");
                    ready.send(()).unwrap();
                    let mut after = Vec::new();
                    client.read_to_end(&mut after).await.unwrap();
                    assert!(after.is_empty(), "{after:?}");
                });

                // A complete response proves that the listener accepted this socket and
                // its connection task ran before shutdown is requested.
                tokio::select! {
                    result = &mut server => panic!("server stopped before shutdown: {result:?}"),
                    result = received => result.unwrap(),
                }
                stop.send(()).unwrap();
                let (server, client) = tokio::join!(server, client);
                server.unwrap();
                client.unwrap();
            })
            .await
            .expect("shutdown must close the idle connection and finish the server promptly");
        });
}

struct ControlledBody(mpsc::Receiver<Bytes>);

impl hyper::body::Body for ControlledBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.get_mut()
            .0
            .poll_recv(cx)
            .map(|next| next.map(|bytes| Ok(Frame::data(bytes))))
    }
}

#[test]
fn shutdown_waits_for_an_inflight_response_to_finish() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            timeout(TEST_TIMEOUT, async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let (chunks, body) = mpsc::channel(1);
                chunks.send(Bytes::from_static(b"first ")).await.unwrap();
                let body = Arc::new(Mutex::new(Some(Body::new(ControlledBody(body)))));
                let app = Router::new().route(
                    "/",
                    get(move || {
                        let body = body.lock().unwrap().take().unwrap();
                        async move {
                            Response::builder()
                                .header(CONTENT_LENGTH, "14")
                                .body(body)
                                .unwrap()
                        }
                    }),
                );
                let (stop, stopped) = oneshot::channel();
                let (observed, mut shutdown_observed) = oneshot::channel();
                let server = serve_with_shutdown(listener, app, async {
                    stopped.await.unwrap();
                    observed.send(()).unwrap();
                });
                tokio::pin!(server);

                let (started, response_started) = oneshot::channel();
                let client = tokio::spawn(async move {
                    let mut socket = TcpStream::connect(address).await.unwrap();
                    socket
                        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                        .await
                        .unwrap();
                    let mut client = BufReader::new(socket);
                    read_head(&mut client).await;
                    let mut first = [0; 6];
                    client.read_exact(&mut first).await.unwrap();
                    assert_eq!(&first, b"first ");
                    started.send(()).unwrap();
                    let mut remaining = Vec::new();
                    client.read_to_end(&mut remaining).await.unwrap();
                    assert_eq!(remaining, b"complete");
                });

                tokio::select! {
                    result = &mut server => panic!("server stopped before shutdown: {result:?}"),
                    result = response_started => result.unwrap(),
                }
                stop.send(()).unwrap();
                // Poll the actual server future while the final body chunk is withheld.
                // The acknowledgement proves it consumed shutdown; Pending proves it
                // still waits for the accepted response, without timing assumptions.
                poll_fn(|cx| match server.as_mut().poll(cx) {
                    Poll::Pending => Poll::Ready(()),
                    Poll::Ready(result) => panic!("server did not drain its response: {result:?}"),
                })
                .await;
                shutdown_observed.try_recv().unwrap();

                chunks.send(Bytes::from_static(b"complete")).await.unwrap();
                drop(chunks);
                let (server, client) = tokio::join!(server, client);
                server.unwrap();
                client.unwrap();
            })
            .await
            .expect("shutdown must finish the in-flight response and then drain its connection");
        });
}
