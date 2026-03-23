use port_authority_core::proto::{InspectResponse, ReservationEvent, ReservationInfo};
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

/// Print a watch event in human-readable format.
pub fn print_watch_event(event: &ReservationEvent) {
    let ts = event
        .timestamp
        .as_ref()
        .map(|t| format_timestamp(t))
        .unwrap_or_else(|| "—".to_string());

    let old = state_name(event.old_state);
    let new = state_name(event.new_state);
    let id_short = if event.reservation_id.len() > 6 {
        &event.reservation_id[..6]
    } else {
        &event.reservation_id
    };

    match &event.message {
        Some(msg) => println!("[{}] {} {} -> {} ({})", ts, id_short, old, new, msg),
        None => println!("[{}] {} {} -> {}", ts, id_short, old, new),
    }
}

/// Print a watch event as NDJSON.
pub fn print_watch_event_json(event: &ReservationEvent) {
    let ts = event
        .timestamp
        .as_ref()
        .map(|t| t.seconds)
        .unwrap_or(0);

    let json = serde_json::json!({
        "reservation_id": event.reservation_id,
        "old_state": state_name(event.old_state),
        "new_state": state_name(event.new_state),
        "timestamp": ts,
        "message": event.message,
    });
    println!("{}", json);
}

/// Format a proto Timestamp as "YYYY-MM-DD HH:MM:SS".
fn format_timestamp(ts: &Timestamp) -> String {
    let secs = ts.seconds as u64;
    // Simple UTC formatting
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    // Convert days since epoch to date
    let (year, month, day) = days_to_date(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, mins, s
    )
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_date(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let month_lengths: [u64; 12] = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &ml in &month_lengths {
        if days < ml {
            break;
        }
        days -= ml;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
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
