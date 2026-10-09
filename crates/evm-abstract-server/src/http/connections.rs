//! Small bounded I/O pool, separate from CPU analysis capacity.

use super::{ServerError, api_error, error_reply, serve_connection};
use crate::jobs;
use evm_abstract_protocol::ApiErrorCode;
use std::{
    net::TcpStream,
    path::Path,
    sync::{
        Arc,
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
};

pub(super) struct Pool {
    workers: Vec<SyncSender<TcpStream>>,
    next: usize,
}

impl Pool {
    pub(super) fn new(assets: &Path, jobs: jobs::Pool) -> Result<Self, ServerError> {
        let mut workers = Vec::new();
        let assets = Arc::new(assets.to_owned());
        for index in 0..4 {
            let (sender, receiver) = mpsc::sync_channel::<TcpStream>(8);
            let worker_assets = Arc::clone(&assets);
            let worker_jobs = jobs.clone();
            thread::Builder::new()
                .name(format!("http-{index}"))
                .spawn(move || {
                    while let Ok(stream) = receiver.recv() {
                        if let Err(error) = serve_connection(stream, &worker_assets, &worker_jobs) {
                            eprintln!("{error}");
                        }
                    }
                })?;
            workers.push(sender);
        }
        Ok(Self { workers, next: 0 })
    }

    pub(super) fn accept(&mut self, mut stream: TcpStream) -> Result<(), ServerError> {
        for offset in 0..self.workers.len() {
            let index = (self.next + offset) % self.workers.len();
            let worker = &self.workers[index];
            match worker.try_send(stream) {
                Ok(()) => {
                    self.next = (index + 1) % self.workers.len();
                    return Ok(());
                }
                Err(TrySendError::Full(returned) | TrySendError::Disconnected(returned)) => {
                    stream = returned
                }
            }
        }
        stream.set_write_timeout(Some(std::time::Duration::from_millis(100)))?;
        error_reply(
            &mut stream,
            api_error(ApiErrorCode::QueueFull, "HTTP connection capacity is full"),
        )
    }
}
