use port_authority_core::proto::port_broker_server::{PortBroker, PortBrokerServer};
use port_authority_core::proto::{
    InspectRequest, InspectResponse, ListRequest, ListResponse, ReleaseRequest, ReleaseResponse,
    ReservationEvent, ReserveRequest, ReserveResponse, WatchRequest,
};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UnixListener;
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};
use tracing::info;

/// The daemon's start time, used for uptime reporting.
pub struct PortBrokerService {
    started_at: Instant,
}

impl PortBrokerService {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

#[tonic::async_trait]
impl PortBroker for PortBrokerService {
    async fn reserve(
        &self,
        _request: Request<ReserveRequest>,
    ) -> Result<Response<ReserveResponse>, Status> {
        Err(Status::unimplemented("reserve not yet implemented"))
    }

    async fn release(
        &self,
        _request: Request<ReleaseRequest>,
    ) -> Result<Response<ReleaseResponse>, Status> {
        Err(Status::unimplemented("release not yet implemented"))
    }

    async fn list(&self, _request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        // Return empty list for now — proves the round-trip works
        Ok(Response::new(ListResponse {
            reservations: vec![],
        }))
    }

    async fn inspect(
        &self,
        _request: Request<InspectRequest>,
    ) -> Result<Response<InspectResponse>, Status> {
        Err(Status::unimplemented("inspect not yet implemented"))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<ReservationEvent, Status>>;

    async fn watch(
        &self,
        _request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        Err(Status::unimplemented("watch not yet implemented (Phase 2)"))
    }
}

/// Wrapper around UnixStream that implements tonic's Connected trait.
#[derive(Debug)]
pub struct UnixStream(pub tokio::net::UnixStream);

impl Connected for UnixStream {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncRead for UnixStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

/// Start the gRPC server on the given Unix listener.
pub async fn serve(uds: UnixListener) -> anyhow::Result<()> {
    let service = PortBrokerService::new();

    info!("gRPC server starting");

    let incoming = async_stream::stream! {
        loop {
            match uds.accept().await {
                Ok((stream, _addr)) => yield Ok::<_, std::io::Error>(UnixStream(stream)),
                Err(e) => yield Err(e),
            }
        }
    };

    tonic::transport::Server::builder()
        .add_service(PortBrokerServer::new(service))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
