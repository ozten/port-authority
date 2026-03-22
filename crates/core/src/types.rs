use serde::{Deserialize, Serialize};
use std::fmt;

/// Reservation lifecycle states, mirroring the protobuf ReservationState enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReservationState {
    Pending,
    Active,
    Failed,
    Released,
}

impl ReservationState {
    /// Convert from the protobuf i32 representation.
    pub fn from_proto(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Pending),
            2 => Some(Self::Active),
            3 => Some(Self::Failed),
            4 => Some(Self::Released),
            _ => None,
        }
    }

    /// Convert to the protobuf i32 representation.
    pub fn to_proto(self) -> i32 {
        match self {
            Self::Pending => 1,
            Self::Active => 2,
            Self::Failed => 3,
            Self::Released => 4,
        }
    }

    /// Convert from the SQLite TEXT representation.
    pub fn from_sql(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "failed" => Some(Self::Failed),
            "released" => Some(Self::Released),
            _ => None,
        }
    }

    /// Convert to the SQLite TEXT representation.
    pub fn as_sql(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Failed => "failed",
            Self::Released => "released",
        }
    }
}

impl fmt::Display for ReservationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_sql())
    }
}
