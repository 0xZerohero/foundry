//! Contains the code to launch an ethereum RPC-Server
use crate::EthApi;
use anvil_server::{ipc::IpcEndpoint, AnvilServer, ServerConfig};
use futures::StreamExt;
use handler::{HttpEthRpcHandler, PubSubEthRpcHandler};
use hyper::server::{accept::Accept, conn::AddrIncoming};
use std::{
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{io, task::JoinHandle};
use tracing::trace;

mod handler;

pub mod error;

/// Configures an [axum::Server] that handles [EthApi] related JSON-RPC calls via HTTP and WS
pub fn serve(
    addrs: &[SocketAddr],
    api: EthApi,
    config: ServerConfig,
) -> (Vec<SocketAddr>, AnvilServer<CombinedIncoming>) {
    let combined_streams = CombinedIncoming::from_socket_addrs(addrs);
    let http = HttpEthRpcHandler::new(api.clone());
    let ws = PubSubEthRpcHandler::new(api);
    (
        combined_streams.local_addrs(),
        anvil_server::serve_http_ws(combined_streams, config, http, ws),
    )
}

/// Launches an ipc server at the given path in a new task
///
/// # Panics
///
/// if setting up the ipc connection was unsuccessful
pub fn spawn_ipc(api: EthApi, path: impl Into<String>) -> JoinHandle<io::Result<()>> {
    try_spawn_ipc(api, path).expect("failed to establish ipc connection")
}

/// Launches an ipc server at the given path in a new task
pub fn try_spawn_ipc(
    api: EthApi,
    path: impl Into<String>,
) -> io::Result<JoinHandle<io::Result<()>>> {
    let path = path.into();
    let handler = PubSubEthRpcHandler::new(api);
    let ipc = IpcEndpoint::new(handler, path);
    let incoming = ipc.incoming()?;

    let task = tokio::task::spawn(async move {
        tokio::pin!(incoming);
        while let Some(stream) = incoming.next().await {
            trace!(target: "ipc", "new ipc connection");
            tokio::task::spawn(stream);
        }
        Ok(())
    });

    Ok(task)
}

/// Combines multiple AddrIncoming into single stream
pub struct CombinedIncoming {
    streams: Vec<AddrIncoming>,
}

impl Accept for CombinedIncoming {
    type Conn = <AddrIncoming as Accept>::Conn;
    type Error = <AddrIncoming as Accept>::Error;

    fn poll_accept(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        for stream in &mut self.streams {
            if let Poll::Ready(value) = Pin::new(stream).poll_accept(cx) {
                return Poll::Ready(value)
            }
        }

        Poll::Pending
    }
}

impl CombinedIncoming {
    fn from_socket_addrs(addrs: &[SocketAddr]) -> Self {
        let streams: Vec<AddrIncoming> = addrs
            .into_iter()
            .map(|addr| {
                AddrIncoming::bind(&addr).unwrap_or_else(|e| {
                    panic!("error binding to {}: {}", addr, e);
                })
            })
            .collect();

        Self { streams }
    }

    /// Get the local addresses bound to the listeners.
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.streams.iter().map(|stream| stream.local_addr()).collect()
    }
}
