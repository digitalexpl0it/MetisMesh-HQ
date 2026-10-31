//! Pair a MeshCore companion before the GATT session starts.
//!
//! MeshCore BLE uses a six-digit passkey. Boards with a screen show it on
//! boot. Screenless boards default to 123456 unless the operator changed it.
//! On Linux we register a temporary BlueZ agent that answers with the PIN the
//! operator typed in the window.

/// Validate the MeshCore Bluetooth PIN. Custom pins are six digits from 100000
/// to 999999; the common screenless default 123456 is included.
pub fn parse_meshcore_pin(raw: &str) -> Result<u32, String> {
    let trimmed = raw.trim();
    if trimmed.len() != 6 || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Err("Enter the six-digit Bluetooth PIN from the radio.".into());
    }
    let pin: u32 = trimmed
        .parse()
        .map_err(|_| "Enter the six-digit Bluetooth PIN from the radio.".to_string())?;
    if !(100_000..=999_999).contains(&pin) {
        return Err("MeshCore Bluetooth PINs are six digits from 100000 to 999999.".into());
    }
    Ok(pin)
}

/// Pair and trust the radio when the platform supports it. Already-paired
/// devices are left alone. The PIN is used only for BlueZ pairing prompts.
pub async fn ensure_paired(target: &str, pin: u32) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::ensure_paired(target, pin).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (target, pin);
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;
    use std::time::Duration;

    use bluer::agent::{
        Agent, RequestAuthorization, RequestConfirmation, RequestPasskey, RequestPinCode,
    };
    use bluer::{Adapter, Address, Session};
    use futures_util::StreamExt;

    pub async fn ensure_paired(target: &str, pin: u32) -> Result<(), String> {
        let pin_code = format!("{pin:06}");
        let pin = Arc::new(pin);
        let pin_code = Arc::new(pin_code);

        let session = Session::new()
            .await
            .map_err(|err| format!("Bluetooth session failed: {err}"))?;

        let agent = Agent {
            request_default: true,
            request_passkey: Some(Box::new({
                let pin = pin.clone();
                move |_req: RequestPasskey| {
                    let pin = *pin;
                    Box::pin(async move { Ok(pin) })
                }
            })),
            request_pin_code: Some(Box::new({
                let pin_code = pin_code.clone();
                move |_req: RequestPinCode| {
                    let pin_code = (*pin_code).clone();
                    Box::pin(async move { Ok(pin_code) })
                }
            })),
            request_confirmation: Some(Box::new(|_req: RequestConfirmation| {
                Box::pin(async move { Ok(()) })
            })),
            request_authorization: Some(Box::new(|_req: RequestAuthorization| {
                Box::pin(async move { Ok(()) })
            })),
            authorize_service: Some(Box::new(|_req| Box::pin(async move { Ok(()) }))),
            ..Default::default()
        };
        // Keep the handle alive through pairing.
        let _agent = session
            .register_agent(agent)
            .await
            .map_err(|err| format!("Could not register the Bluetooth PIN agent: {err}"))?;

        let adapter = session
            .default_adapter()
            .await
            .map_err(|err| format!("No Bluetooth adapter: {err}"))?;
        let _ = adapter.set_powered(true).await;

        let address = resolve_address(&adapter, target).await?;
        let device = adapter
            .device(address)
            .map_err(|err| format!("Bluetooth device {address} is not available: {err}"))?;

        if device.is_paired().await.unwrap_or(false) {
            let _ = device.set_trusted(true).await;
            return Ok(());
        }

        device
            .pair()
            .await
            .map_err(|err| {
                format!(
                    "Pairing failed: {err}. Check the PIN, forget the device in system Bluetooth if it was paired before, and disconnect the phone app."
                )
            })?;
        let _ = device.set_trusted(true).await;
        Ok(())
    }

    async fn resolve_address(adapter: &Adapter, target: &str) -> Result<Address, String> {
        let trimmed = target.trim();
        if let Ok(address) = trimmed.parse::<Address>() {
            return Ok(address);
        }

        let _ = adapter
            .set_discovery_filter(bluer::DiscoveryFilter {
                transport: bluer::DiscoveryTransport::Le,
                ..Default::default()
            })
            .await;
        let discover = adapter
            .discover_devices()
            .await
            .map_err(|err| format!("Bluetooth discovery failed: {err}"))?;
        tokio::pin!(discover);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        let target_lower = trimmed.to_ascii_lowercase();
        while tokio::time::Instant::now() < deadline {
            let next = tokio::time::timeout_at(deadline, discover.next()).await;
            match next {
                Ok(Some(event)) => {
                    let address = match event {
                        bluer::AdapterEvent::DeviceAdded(address) => address,
                        _ => continue,
                    };
                    if device_matches(adapter, address, &target_lower).await {
                        return Ok(address);
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }

        for address in adapter
            .device_addresses()
            .await
            .map_err(|err| format!("Could not list Bluetooth devices: {err}"))?
        {
            if device_matches(adapter, address, &target_lower).await {
                return Ok(address);
            }
        }

        Err(format!(
            "Could not find Bluetooth device '{trimmed}' to pair. Scan again, then connect."
        ))
    }

    async fn device_matches(adapter: &Adapter, address: Address, target_lower: &str) -> bool {
        let Ok(device) = adapter.device(address) else {
            return false;
        };
        if address.to_string().eq_ignore_ascii_case(target_lower) {
            return true;
        }
        if let Ok(Some(name)) = device.name().await {
            if name.eq_ignore_ascii_case(target_lower) {
                return true;
            }
        }
        if let Ok(alias) = device.alias().await {
            if !alias.is_empty() && alias.eq_ignore_ascii_case(target_lower) {
                return true;
            }
        }
        false
    }

}

#[cfg(test)]
mod tests {
    use super::parse_meshcore_pin;

    #[test]
    fn accepts_default_and_custom_pins() {
        assert_eq!(parse_meshcore_pin("123456").unwrap(), 123456);
        assert_eq!(parse_meshcore_pin(" 654321 ").unwrap(), 654321);
    }

    #[test]
    fn rejects_short_or_non_digit_pins() {
        assert!(parse_meshcore_pin("12345").is_err());
        assert!(parse_meshcore_pin("abcdef").is_err());
        assert!(parse_meshcore_pin("012345").is_err());
    }
}
