use thiserror::Error;

#[derive(Error, Debug)]
pub enum PortError {
    #[error("port {0} is already reserved by {1}")]
    PortUnavailable(u16, String),

    #[error("port {0} unavailable and exact_only requested")]
    ExactPortUnavailable(u16),

    #[error("no ports available in range {0}-{1}")]
    PortRangeExhausted(u16, u16),

    #[error("reservation {0} not found")]
    ReservationNotFound(String),

    #[error("VM {0} not found in SSH config")]
    VmNotConfigured(String),

    #[error("SSH connection to {0} failed: {1}")]
    SshConnectionFailed(String, String),

    #[error("database error: {0}")]
    Database(String),

    #[error("invalid state transition from {0} to {1}")]
    InvalidTransition(String, String),
}

impl From<PortError> for tonic::Status {
    fn from(e: PortError) -> tonic::Status {
        match &e {
            PortError::PortUnavailable(..) | PortError::ExactPortUnavailable(_) => {
                tonic::Status::already_exists(e.to_string())
            }
            PortError::PortRangeExhausted(..) => tonic::Status::resource_exhausted(e.to_string()),
            PortError::ReservationNotFound(_) => tonic::Status::not_found(e.to_string()),
            PortError::VmNotConfigured(_) => tonic::Status::failed_precondition(e.to_string()),
            PortError::SshConnectionFailed(..) => tonic::Status::unavailable(e.to_string()),
            PortError::Database(_) => tonic::Status::internal(e.to_string()),
            PortError::InvalidTransition(..) => tonic::Status::failed_precondition(e.to_string()),
        }
    }
}
