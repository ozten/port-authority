use port_authority_core::proto::{InspectResponse, ReservationInfo};
use prost_types::Timestamp;
use std::fmt::Debug;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Print any Debug value as JSON-like output.
/// Proto types don't implement Serialize, so we use Debug for now.
pub fn print_json(value: &impl Debug) -> anyhow::Result<()> {
    println!("{:#?}", value);
    Ok(())
}

/// Convert a proto ReservationState i32 to a display name.
pub fn state_name(state: i32) -> &'static str {
    match state {
        1 => "pending",
        2 => "active",
        3 => "failed",
        4 => "released",
        _ => "unknown",
    }
}

/// Print a table of reservations.
pub fn print_reservation_table(reservations: &[ReservationInfo]) {
    if reservations.is_empty() {
        println!("No active reservations");
        return;
    }

    // Header
    println!(
        "{:<7} {:<16} {:<14} {:<9} {:<10} {}",
        "PORT", "OWNER", "TARGET", "STATE", "AGE", "LEASE"
    );

    for r in reservations {
        let target = format!("{}:{}", r.target_host, r.target_port);
        let age = r
            .created_at
            .as_ref()
            .map(|ts| format_age(ts))
            .unwrap_or_else(|| "—".to_string());
        let lease = r
            .lease_seconds
            .map(|s| format_duration(s as u64))
            .unwrap_or_else(|| "—".to_string());

        println!(
            "{:<7} {:<16} {:<14} {:<9} {:<10} {}",
            r.assigned_port,
            r.owner,
            target,
            state_name(r.state),
            age,
            lease,
        );
    }
}

/// Print detailed inspect output.
pub fn print_inspect(resp: &InspectResponse) {
    if let Some(r) = &resp.reservation {
        println!("Reservation: {}", r.id);
        println!("  Owner:       {}", r.owner);
        println!("  Port:        {}", r.assigned_port);
        if r.requested_port != 0 {
            println!("  Requested:   {}", r.requested_port);
        }
        println!("  Target:      {}:{}", r.target_host, r.target_port);
        println!("  State:       {}", state_name(r.state));
        if let Some(ts) = &r.created_at {
            println!("  Created:     {} ago", format_age(ts));
        }
    }

    if let Some(health) = &resp.tunnel_health {
        println!("  Tunnel:");
        println!("    Alive:     {}", health.alive);
        println!(
            "    Uptime:    {}",
            format_duration(health.uptime_seconds as u64)
        );
        println!("    Reconnects: {}", health.reconnect_count);
    }
}

fn format_age(ts: &Timestamp) -> String {
    let created = UNIX_EPOCH + Duration::new(ts.seconds as u64, ts.nanos as u32);
    let now = SystemTime::now();
    match now.duration_since(created) {
        Ok(d) => format_duration(d.as_secs()),
        Err(_) => "—".to_string(),
    }
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        format!("{}h {}m", h, m)
    } else {
        let d = secs / 86400;
        let h = (secs % 86400) / 3600;
        format!("{}d {}h", d, h)
    }
}
