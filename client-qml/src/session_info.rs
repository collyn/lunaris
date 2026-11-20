//! What a session tells the user beside the picture: who connects (the
//! name the person at the remote computer is asked about), why a connection
//! was refused, and what the agent says of the computer it shows (its
//! displays and the delay to it).

fn hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default()
}

/// `&name=…` for a client connection: who connects, as the person at the
/// remote computer is asked about (agents ignore it with the password).
pub fn client_name_query() -> String {
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default();
    let who = if user.is_empty() { hostname() } else { format!("{}@{}", user, hostname()) };
    format!("&name={}", url::form_urlencoded::byte_serialize(who.as_bytes()).collect::<String>())
}

/// What to tell the user for an `AccessRequest` status: None while the
/// connection may go on ("waiting", "accepted").
pub fn access_refusal(status: &str) -> Option<String> {
    match status {
        "waiting" | "accepted" => None,
        "denied" => Some("The person at the remote computer declined the connection.".into()),
        "timeout" => Some("Nobody accepted the connection at the remote computer in time.".into()),
        "unattended" => Some("Nobody can accept connections at the remote computer: enter its password.".into()),
        _ => Some("The remote computer refused the connection.".into()),
    }
}

// ---- The displays of the computer being shown ----

/// The displays of the source being shown, when `data` is the message that
/// lists them on the general data channel
/// (`{"type":"source_displays","displays":[{id,name,width,height}],"current":"<id>"}`):
/// the list as JSON for the stream settings, and the current display's id.
pub fn source_displays(data: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(data).ok()?;
    // Cursor updates come on the same channel many times a second.
    if !text.contains("source_displays") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("type").and_then(|v| v.as_str()) != Some("source_displays") {
        return None;
    }
    let text_of = |v: Option<&serde_json::Value>| match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let displays: Vec<serde_json::Value> = value
        .get("displays")?
        .as_array()?
        .iter()
        .filter_map(|d| {
            let id = text_of(d.get("id"));
            if id.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "id": id,
                "name": text_of(d.get("name")),
                "width": d.get("width").and_then(|v| v.as_u64()).unwrap_or(0),
                "height": d.get("height").and_then(|v| v.as_u64()).unwrap_or(0),
            }))
        })
        .collect();
    Some((serde_json::Value::Array(displays).to_string(), text_of(value.get("current"))))
}

/// The delay between the agent and the computer it shows, there and back,
/// in milliseconds (with a fraction: the agent tells tenths), when `data`
/// is the message that tells it on the general data channel
/// (`{"type":"source_latency","ms":12.3}`, agent connector/media.rs
/// `latency_message`). Only sessions that show another computer through a
/// connector send it.
pub fn source_latency_ms(data: &[u8]) -> Option<f64> {
    let text = std::str::from_utf8(data).ok()?;
    // Cursor updates come on the same channel many times a second.
    if !text.contains("source_latency") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("type").and_then(|v| v.as_str()) != Some("source_latency") {
        return None;
    }
    let ms = value.get("ms")?.as_f64()?;
    // A delay: not negative, and not beyond what the bridge keeps (microseconds in 32 bits).
    (ms.is_finite() && ms >= 0.0).then_some(ms.min(4_000_000.0))
}

/// The ping shown: the viewer's round trip to the agent plus the agent's
/// to the computer it shows (0 when the agent shows its own screen).
pub fn shown_ping_ms(agent_round_trip_ms: f64, source_latency_ms: f64) -> f64 {
    agent_round_trip_ms + source_latency_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_told_and_waiting_is_not() {
        assert!(access_refusal("waiting").is_none());
        assert!(access_refusal("accepted").is_none());
        assert!(access_refusal("denied").is_some());
        assert!(access_refusal("something else").is_some());
        assert!(client_name_query().starts_with("&name="));
    }

    #[test]
    fn the_displays_of_a_source() {
        let message = br#"{"type":"source_displays","current":"1","displays":[{"id":"0","name":"Display 1","width":1920,"height":1080},{"id":"1","name":"DP-2","width":2560,"height":1440}]}"#;
        let (displays, current) = source_displays(message).unwrap();
        assert_eq!(current, "1");
        let displays: serde_json::Value = serde_json::from_str(&displays).unwrap();
        assert_eq!(displays.as_array().unwrap().len(), 2);
        assert_eq!(displays[1]["name"], "DP-2");
        assert_eq!(displays[1]["width"], 2560);
        assert!(source_displays(br#"{"type":"host_cursor","x":1,"y":2}"#).is_none());
        assert!(source_displays(b"not json source_displays").is_none());
    }

    #[test]
    fn the_delay_to_a_source_is_added_to_the_ping() {
        assert_eq!(source_latency_ms(br#"{"type":"source_latency","ms":12}"#), Some(12.0));
        assert_eq!(source_latency_ms(br#"{"type":"source_latency","ms":0.4}"#), Some(0.4));
        assert_eq!(source_latency_ms(br#"{"ms":0,"type":"source_latency"}"#), Some(0.0));
        // Other messages of the channel, and what is not a count of milliseconds.
        assert_eq!(source_latency_ms(br#"{"type":"host_cursor","x":1,"y":2}"#), None);
        assert_eq!(source_latency_ms(br#"{"type":"source_displays","displays":[],"current":"0"}"#), None);
        assert_eq!(source_latency_ms(br#"{"type":"source_latency"}"#), None);
        assert_eq!(source_latency_ms(br#"{"type":"source_latency","ms":-3}"#), None);
        assert_eq!(source_latency_ms(br#"{"type":"source_latency","ms":"12"}"#), None);
        assert_eq!(source_latency_ms(b"source_latency not json"), None);
        // And it is not taken for the list of displays.
        assert!(source_displays(br#"{"type":"source_latency","ms":12}"#).is_none());

        assert_eq!(shown_ping_ms(0.1, 12.0), 12.1);
        assert_eq!(shown_ping_ms(0.25, 0.5), 0.75);
        // A session of the agent's own screen: its ping as before.
        assert_eq!(shown_ping_ms(7.5, 0.0), 7.5);
    }
}
