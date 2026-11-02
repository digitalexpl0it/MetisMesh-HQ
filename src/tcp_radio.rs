//! MeshCore companion over TCP (Wi-Fi / Ethernet). Default port 5000.

/// Official MeshCore Wi-Fi / Ethernet companion TCP port.
pub const DEFAULT_TCP_PORT: u16 = 5000;

/// Normalize operator input to `host:port` for storage and reconnect.
pub fn normalize_tcp_address(raw: &str) -> Result<String, String> {
    let (host, port) = parse_tcp_address(raw)?;
    Ok(format!("{host}:{port}"))
}

/// Parse `host`, `host:port`, or `[ipv6]:port` into host + port.
pub fn parse_tcp_address(raw: &str) -> Result<(String, u16), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Enter the companion host or IP address.".into());
    }

    // Bracketed IPv6: [fe80::1]:5000 or [fe80::1]
    if let Some(rest) = trimmed.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Err("IPv6 address needs closing brackets, e.g. [fe80::1]:5000.".into());
        };
        let host = rest[..end].trim();
        if host.is_empty() {
            return Err("Enter a host inside the brackets.".into());
        }
        let after = rest[end + 1..].trim();
        let port = if after.is_empty() {
            DEFAULT_TCP_PORT
        } else if let Some(p) = after.strip_prefix(':') {
            parse_port(p)?
        } else {
            return Err("After an IPv6 address use :port, e.g. [fe80::1]:5000.".into());
        };
        return Ok((host.to_string(), port));
    }

    // host:port — only split on the last colon when it looks like a port
    // (not an unbracketed IPv6 which contains many colons).
    if trimmed.matches(':').count() == 1 {
        if let Some((host, port_raw)) = trimmed.rsplit_once(':') {
            let host = host.trim();
            if host.is_empty() {
                return Err("Enter the companion host or IP address.".into());
            }
            return Ok((host.to_string(), parse_port(port_raw)?));
        }
    }

    // Bare hostname / IPv4 / unbracketed IPv6 → default port.
    Ok((trimmed.to_string(), DEFAULT_TCP_PORT))
}

fn parse_port(raw: &str) -> Result<u16, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(DEFAULT_TCP_PORT);
    }
    let port: u16 = t
        .parse()
        .map_err(|_| format!("Port `{t}` is not a number between 1 and 65535."))?;
    if port == 0 {
        return Err("Port must be between 1 and 65535.".into());
    }
    Ok(port)
}

/// Short label for the pool list when the operator did not name the radio.
pub fn tcp_fallback_label(address: &str) -> String {
    format!("TCP {address}")
}

pub fn clarify_tcp_connect_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("connection refused") || lower.contains("actively refused") {
        return format!(
            "{raw} Is the companion on Wi-Fi/Ethernet companion firmware and listening on TCP (default port {DEFAULT_TCP_PORT})?"
        );
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return format!(
            "{raw} Check the IP, that HQ is on the same LAN, and that nothing is blocking port {DEFAULT_TCP_PORT}."
        );
    }
    if lower.contains("network is unreachable")
        || lower.contains("no route to host")
        || lower.contains("name or service not known")
        || lower.contains("failed to lookup")
        || lower.contains("nodename nor servname")
    {
        return format!("{raw} Check the host name or IP address.");
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_only_gets_default_port() {
        assert_eq!(
            parse_tcp_address("192.168.1.50").unwrap(),
            ("192.168.1.50".into(), 5000)
        );
        assert_eq!(
            normalize_tcp_address("companion.local").unwrap(),
            "companion.local:5000"
        );
    }

    #[test]
    fn host_and_port() {
        assert_eq!(
            parse_tcp_address("10.0.0.2:5000").unwrap(),
            ("10.0.0.2".into(), 5000)
        );
        assert_eq!(
            normalize_tcp_address("mesh.lan:4000").unwrap(),
            "mesh.lan:4000"
        );
    }

    #[test]
    fn rejects_empty_and_bad_port() {
        assert!(parse_tcp_address("").is_err());
        assert!(parse_tcp_address("   ").is_err());
        assert!(parse_tcp_address("host:0").is_err());
        assert!(parse_tcp_address("host:abc").is_err());
        assert!(parse_tcp_address("host:99999").is_err());
    }

    #[test]
    fn bracketed_ipv6() {
        assert_eq!(
            parse_tcp_address("[fe80::1]").unwrap(),
            ("fe80::1".into(), 5000)
        );
        assert_eq!(
            parse_tcp_address("[fe80::1]:5000").unwrap(),
            ("fe80::1".into(), 5000)
        );
    }
}
