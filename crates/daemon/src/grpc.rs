use crate::broker::{Broker, Reservation};
use crate::tunnel::TunnelManager;
use port_authority_core::proto::port_broker_server::{PortBroker, PortBrokerServer};
use port_authority_core::proto::{
    InspectRequest, InspectResponse, ListRequest, ListResponse, ReleaseRequest, ReleaseResponse,
    ReservationEvent, ReservationInfo, ReserveRequest, ReserveResponse, TunnelHealth, WatchRequest,
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
use tracing::{info, warn};

fn validate_owner(owner: &str) -> Result<(), Status> {
    if owner.is_empty() || owner.len() > 128 {
        return Err(Status::invalid_argument("owner must be 1-128 characters"));
    }
    if !owner
        .chars()
        .all(|c| c.is_alphanumeric() || ":-_.".contains(c))
    {
        return Err(Status::invalid_argument(
            "owner contains invalid characters",
        ));
    }
    Ok(())
}

fn validate_target_host(host: &str) -> Result<(), Status> {
    if host.is_empty() || host.len() > 253 {
        return Err(Status::invalid_argument(
            "target_host must be 1-253 characters",
        ));
    }
    Ok(())
}

pub struct PortBrokerService {
    broker: Arc<Mutex<Broker>>,
    tunnel_manager: Arc<Mutex<TunnelManager>>,
    #[allow(dead_code)]
    started_at: Instant,
}

impl PortBrokerService {
    pub fn new(broker: Arc<Mutex<Broker>>, tunnel_manager: Arc<Mutex<TunnelManager>>) -> Self {
        Self {
            broker,
            tunnel_manager,
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
    let dt = chrono_parse_naive(s)?;
    Some(Timestamp {
        seconds: dt,
        nanos: 0,
    })
}

/// Minimal datetime parsing without chrono dependency.
/// Parses "YYYY-MM-DD HH:MM:SS" to Unix timestamp.
fn chrono_parse_naive(s: &str) -> Option<i64> {
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

        validate_owner(&req.owner)?;
        validate_target_host(&req.target_host)?;
        if req.target_port == 0 || req.target_port > 65535 {
            return Err(Status::invalid_argument("target_port must be 1-65535"));
        }
        if let Some(port) = req.preferred_port {
            if port == 0 || port > 65535 {
                return Err(Status::invalid_argument("preferred_port must be 1-65535"));
            }
        }

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

        // For VM reservations in pending state, start the SSH tunnel
        let mut final_state = ReservationState::from_sql(&reservation.state)
            .map(|s| s.to_proto())
            .unwrap_or(0);

        if let Some(vm_name) = Broker::vm_name_from_owner(&reservation.owner) {
            if reservation.state == ReservationState::Pending.as_sql() {
                let assigned_port = reservation.assigned_port as u16;
                let target_host = reservation.target_host.clone();
                let target_port = reservation.target_port as u16;
                let reservation_id = reservation.id.clone();
                let vm_name = vm_name.to_string();

                // Release hold listener so the tunnel can bind the port
                broker.release_hold_listener(assigned_port);

                // Drop broker lock before SSH I/O
                drop(broker);

                let mut tm = self.tunnel_manager.lock().await;
                match tm
                    .start_tunnel(
                        &reservation_id,
                        &vm_name,
                        assigned_port,
                        &target_host,
                        target_port,
                    )
                    .await
                {
                    Ok(()) => {
                        // Transition to active
                        let broker = self.broker.lock().await;
                        broker
                            .update_state(&reservation_id, ReservationState::Active)
                            .await
                            .map_err(|e| tonic::Status::from(e))?;
                        final_state = ReservationState::Active.to_proto();
                        info!(id = %reservation_id, "VM tunnel started, reservation active");
                    }
                    Err(e) => {
                        warn!(id = %reservation_id, error = %e, "tunnel start failed");
                        let broker = self.broker.lock().await;
                        let _ = broker
                            .update_state(&reservation_id, ReservationState::Failed)
                            .await;
                        final_state = ReservationState::Failed.to_proto();
                    }
                }

                return Ok(Response::new(ReserveResponse {
                    reservation_id: reservation.id,
                    assigned_port: reservation.assigned_port as u32,
                    state: final_state,
                }));
            }
        }

        Ok(Response::new(ReserveResponse {
            reservation_id: reservation.id,
            assigned_port: reservation.assigned_port as u32,
            state: final_state,
        }))
    }

    async fn release(
        &self,
        request: Request<ReleaseRequest>,
    ) -> Result<Response<ReleaseResponse>, Status> {
        let req = request.into_inner();
        let mut broker = self.broker.lock().await;

        // Get reservation info before releasing (for tunnel teardown)
        let reservation = match &req.identifier {
            Some(port_authority_core::proto::release_request::Identifier::ReservationId(id)) => {
                broker.get_reservation_by_id(id).await.ok()
            }
            Some(port_authority_core::proto::release_request::Identifier::Port(port)) => {
                broker.get_reservation_by_port(*port as u16).await.ok()
            }
            None => return Err(Status::invalid_argument("specify reservation_id or port")),
        };

        let result = match &req.identifier {
            Some(port_authority_core::proto::release_request::Identifier::ReservationId(id)) => {
                broker.release_by_id(id).await
            }
            Some(port_authority_core::proto::release_request::Identifier::Port(port)) => {
                broker.release_by_port(*port as u16).await
            }
            None => unreachable!(),
        };

        // Drop broker lock before touching tunnel manager
        drop(broker);

        // Tear down any associated tunnel
        if let Some(r) = reservation {
            if Broker::vm_name_from_owner(&r.owner).is_some() {
                let mut tm = self.tunnel_manager.lock().await;
                tm.stop_tunnel(&r.id);
            }
        }

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

        if let Some(ref owner) = req.owner_filter {
            validate_owner(owner)?;
        }

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

        let is_host = reservation.owner.starts_with("host:");

        // Get real tunnel health for VM reservations
        let health = if is_host {
            TunnelHealth {
                alive: true,
                last_check: None,
                uptime_seconds: 0,
                reconnect_count: 0,
            }
        } else {
            // Drop broker lock before touching tunnel manager
            drop(broker);
            let tm = self.tunnel_manager.lock().await;
            match tm.get_health(&reservation.id).await {
                Some(h) => TunnelHealth {
                    alive: h.alive,
                    last_check: h.last_check.map(|_| {
                        // Use current time as approximate last_check timestamp
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default();
                        Timestamp {
                            seconds: now.as_secs() as i64,
                            nanos: 0,
                        }
                    }),
                    uptime_seconds: h.started_at.elapsed().as_secs() as u32,
                    reconnect_count: h.reconnect_count,
                },
                None => TunnelHealth {
                    alive: reservation.state == ReservationState::Active.as_sql(),
                    last_check: None,
                    uptime_seconds: 0,
                    reconnect_count: reservation.reconnect_count as u32,
                },
            }
        };

        Ok(Response::new(InspectResponse {
            reservation: Some(info),
            tunnel_health: Some(health),
        }))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<ReservationEvent, Status>>;

    async fn watch(
        &self,
        request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.into_inner();
        let owner_filter = req.owner_filter;

        // Subscribe to broker events
        let mut event_rx = {
            let broker = self.broker.lock().await;
            broker.subscribe()
        };

        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        // Apply owner filter if specified
                        if let Some(ref filter) = owner_filter {
                            if !event.owner.starts_with(filter.as_str()) {
                                continue;
                            }
                        }

                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default();

                        let old_state = ReservationState::from_sql(&event.old_state)
                            .map(|s| s.to_proto())
                            .unwrap_or(0);
                        let new_state = ReservationState::from_sql(&event.new_state)
                            .map(|s| s.to_proto())
                            .unwrap_or(0);

                        let proto_event = ReservationEvent {
                            reservation_id: event.reservation_id,
                            old_state,
                            new_state,
                            timestamp: Some(Timestamp {
                                seconds: now.as_secs() as i64,
                                nanos: 0,
                            }),
                            message: event.message,
                        };

                        if tx.send(Ok(proto_event)).await.is_err() {
                            // Client disconnected
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "watch stream lagged, some events were dropped");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
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
pub async fn serve(
    uds: UnixListener,
    broker: Arc<Mutex<Broker>>,
    tunnel_manager: Arc<Mutex<TunnelManager>>,
) -> anyhow::Result<()> {
    let service = PortBrokerService::new(broker, tunnel_manager);

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
