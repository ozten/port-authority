use crate::broker::{Broker, Reservation};
use port_authority_core::proto::port_broker_server::{PortBroker, PortBrokerServer};
use port_authority_core::proto::{
    InspectRequest, InspectResponse, ListRequest, ListResponse, ReleaseRequest, ReleaseResponse,
    ReservationEvent, ReservationInfo, ReservationState as ProtoState, ReserveRequest,
    ReserveResponse, TunnelHealth, WatchRequest,
};
use port_authority_core::types::ReservationState;
use prost_types::Timestamp;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tonic::transport::server::Connected;
use tonic::{Request, Response, Status};
use tracing::info;

pub struct PortBrokerService {
    broker: Arc<Mutex<Broker>>,
    started_at: Instant,
}

impl PortBrokerService {
    pub fn new(broker: Arc<Mutex<Broker>>) -> Self {
        Self {
            broker,
            started_at: Instant::now(),
        }
    }
}

/// Convert a Reservation to a proto ReservationInfo.
fn reservation_to_proto(r: &Reservation) -> ReservationInfo {
    ReservationInfo {
        id: r.id.clone(),
        owner: r.owner.clone(),
        requested_port: r.requested_port.unwrap_or(0) as u32,
        assigned_port: r.assigned_port as u32,
        target_host: r.target_host.clone(),
        target_port: r.target_port as u32,
        state: ReservationState::from_sql(&r.state)
            .map(|s| s.to_proto())
            .unwrap_or(0),
        created_at: parse_sqlite_timestamp(&r.created_at),
        updated_at: parse_sqlite_timestamp(&r.updated_at),
        lease_seconds: r.lease_seconds.map(|s| s as u32),
        expires_at: r.expires_at.as_deref().and_then(parse_sqlite_timestamp),
    }
}

/// Parse a SQLite datetime string like "2026-03-22 01:23:45" into a proto Timestamp.
fn parse_sqlite_timestamp(s: &str) -> Option<Timestamp> {
    // SQLite datetime format: "YYYY-MM-DD HH:MM:SS"
    // We need to convert to Unix epoch seconds
    // Simple parsing: use the fact that SQLite returns UTC
    let dt = chrono_parse_naive(s)?;
    Some(Timestamp {
        seconds: dt,
        nanos: 0,
    })
}

/// Minimal datetime parsing without chrono dependency.
/// Parses "YYYY-MM-DD HH:MM:SS" to Unix timestamp.
fn chrono_parse_naive(s: &str) -> Option<i64> {
    // Split "2026-03-22 01:23:45" into date and time parts
    let parts: Vec<&str> = s.split(' ').collect();
    if parts.len() != 2 {
        return None;
    }

    let date_parts: Vec<u32> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time_parts: Vec<u32> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();

    if date_parts.len() != 3 || time_parts.len() != 3 {
        return None;
    }

    let (year, month, day) = (
        date_parts[0] as i64,
        date_parts[1] as i64,
        date_parts[2] as i64,
    );
    let (hour, min, sec) = (
        time_parts[0] as i64,
        time_parts[1] as i64,
        time_parts[2] as i64,
    );

    // Days from epoch (1970-01-01) — simplified calculation
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize] as i64;
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[tonic::async_trait]
impl PortBroker for PortBrokerService {
    async fn reserve(
        &self,
        request: Request<ReserveRequest>,
    ) -> Result<Response<ReserveResponse>, Status> {
        let req = request.into_inner();
        let mut broker = self.broker.lock().await;

        let reservation = broker
            .reserve(
                &req.owner,
                req.preferred_port.map(|p| p as u16),
                &req.target_host,
                req.target_port as u16,
                req.lease_seconds,
                req.exact_only,
            )
            .await
            .map_err(|e| tonic::Status::from(e))?;

        let state = ReservationState::from_sql(&reservation.state)
            .map(|s| s.to_proto())
            .unwrap_or(0);

        Ok(Response::new(ReserveResponse {
            reservation_id: reservation.id,
            assigned_port: reservation.assigned_port as u32,
            state,
        }))
    }

    async fn release(
        &self,
        request: Request<ReleaseRequest>,
    ) -> Result<Response<ReleaseResponse>, Status> {
        let req = request.into_inner();
        let mut broker = self.broker.lock().await;

        let result = match req.identifier {
            Some(port_authority_core::proto::release_request::Identifier::ReservationId(id)) => {
                broker.release_by_id(&id).await
            }
            Some(port_authority_core::proto::release_request::Identifier::Port(port)) => {
                broker.release_by_port(port as u16).await
            }
            None => return Err(Status::invalid_argument("specify reservation_id or port")),
        };

        match result {
            Ok(()) => Ok(Response::new(ReleaseResponse {
                success: true,
                message: "released".to_string(),
            })),
            Err(e) => Err(tonic::Status::from(e)),
        }
    }

    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let req = request.into_inner();
        let broker = self.broker.lock().await;

        let state_filter = req
            .state_filter
            .and_then(|s| ReservationState::from_proto(s).map(|st| st.as_sql().to_string()));

        let reservations = broker
            .list(req.owner_filter.as_deref(), state_filter.as_deref())
            .await
            .map_err(|e| tonic::Status::from(e))?;

        let infos: Vec<ReservationInfo> = reservations.iter().map(reservation_to_proto).collect();

        Ok(Response::new(ListResponse {
            reservations: infos,
        }))
    }

    async fn inspect(
        &self,
        request: Request<InspectRequest>,
    ) -> Result<Response<InspectResponse>, Status> {
        let req = request.into_inner();
        let broker = self.broker.lock().await;

        let reservation = match req.identifier {
            Some(port_authority_core::proto::inspect_request::Identifier::ReservationId(id)) => {
                broker.get_reservation_by_id(&id).await
            }
            Some(port_authority_core::proto::inspect_request::Identifier::Port(port)) => {
                broker.get_reservation_by_port(port as u16).await
            }
            None => return Err(Status::invalid_argument("specify reservation_id or port")),
        }
        .map_err(|e| tonic::Status::from(e))?;

        let info = reservation_to_proto(&reservation);

        // Tunnel health — for host-side reservations, always report alive
        let is_host = reservation.owner.starts_with("host:");
        let health = TunnelHealth {
            alive: is_host || reservation.state == ReservationState::Active.as_sql(),
            last_check: None,
            uptime_seconds: 0,
            reconnect_count: reservation.reconnect_count as u32,
        };

        Ok(Response::new(InspectResponse {
            reservation: Some(info),
            tunnel_health: Some(health),
        }))
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
pub async fn serve(uds: UnixListener, broker: Arc<Mutex<Broker>>) -> anyhow::Result<()> {
    let service = PortBrokerService::new(broker);

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
