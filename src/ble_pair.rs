//! Pair a MeshCore companion before the GATT session starts.
//!
//! MeshCore BLE uses a six-digit passkey. Boards with a screen show it on
//! boot. Screenless boards default to 123456 unless the operator changed it.
//! Companion firmware (ESP32 and nRF52) ignores every frame until the link is
//! encrypted with a PIN-authenticated bond, so this step is required.
//! On Linux we register a temporary BlueZ agent that answers with the PIN the
//! operator typed in the window. On Windows we use WinRT custom pairing with
//! the same PIN; btleplug never pairs on its own.

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

/// Pair and trust the radio when the platform supports it. Devices already
/// bonded with PIN authentication are left alone. macOS has no pairing API;
/// CoreBluetooth shows its own PIN prompt on first access.
pub async fn ensure_paired(target: &str, pin: u32) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::ensure_paired(target, pin).await
    }
    #[cfg(target_os = "windows")]
    {
        windows_pair::ensure_paired(target, pin).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (target, pin);
        Ok(())
    }
}

/// Parse `AA:BB:CC:DD:EE:FF` (also `-` separated or bare 12 hex digits) into
/// the 48-bit integer WinRT uses, most significant byte first.
#[cfg(any(target_os = "windows", test))]
fn parse_ble_address(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let hex: String = if raw.len() == 17 {
        let parts: Vec<&str> = raw.split([':', '-']).collect();
        if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
            return None;
        }
        parts.concat()
    } else {
        raw.to_string()
    };
    if hex.len() != 12 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(&hex, 16).ok()
}

/// Drop the GATT link to `target` (address or advertised name).
///
/// meshcore-rs `disconnect()` only stops its own tasks; without this the
/// companion stays connected to the laptop and the phone app cannot take over.
pub async fn force_disconnect(target: &str) -> Result<(), String> {
    use btleplug::api::{Central, Manager as _, Peripheral as _};
    use btleplug::platform::Manager;

    let target = target.trim();
    if target.is_empty() {
        return Ok(());
    }
    let manager = Manager::new()
        .await
        .map_err(|err| format!("Bluetooth manager: {err}"))?;
    let adapters = manager
        .adapters()
        .await
        .map_err(|err| format!("Bluetooth adapters: {err}"))?;
    for adapter in adapters {
        let Ok(peripherals) = adapter.peripherals().await else {
            continue;
        };
        for peripheral in peripherals {
            let Ok(Some(props)) = peripheral.properties().await else {
                continue;
            };
            let matches = props.address.to_string().eq_ignore_ascii_case(target)
                || props.local_name.as_deref() == Some(target);
            if !matches {
                continue;
            }
            if peripheral.is_connected().await.unwrap_or(false) {
                peripheral
                    .disconnect()
                    .await
                    .map_err(|err| format!("Bluetooth disconnect: {err}"))?;
            }
            return Ok(());
        }
    }
    Ok(())
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

#[cfg(target_os = "windows")]
mod windows_pair {
    use std::time::Duration;

    use windows::core::{Ref, HSTRING};
    use windows::Devices::Bluetooth::BluetoothLEDevice;
    use windows::Devices::Enumeration::{
        DeviceInformation, DeviceInformationCustomPairing, DeviceInformationPairing,
        DevicePairingKinds, DevicePairingProtectionLevel, DevicePairingRequestedEventArgs,
        DevicePairingResultStatus, DeviceUnpairingResultStatus,
    };
    use windows::Foundation::TypedEventHandler;

    /// Windows does not cancel a pairing ceremony; stop waiting after this.
    const PAIR_TIMEOUT: Duration = Duration::from_secs(45);

    pub async fn ensure_paired(target: &str, pin: u32) -> Result<(), String> {
        let address = super::parse_ble_address(target).ok_or_else(|| {
            format!(
                "'{}' is not a Bluetooth address. Scan, select the radio in the list, then Pair.",
                target.trim()
            )
        })?;
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(address)
            .map_err(|err| format!("Bluetooth lookup failed: {err}"))?
            .await
            .map_err(|_| {
                "Windows cannot see this radio. Power it on, close the phone app, Scan again, then Pair."
                    .to_string()
            })?;
        let device_id = device
            .DeviceId()
            .map_err(|err| format!("Bluetooth device id unavailable: {err}"))?;

        let pairing = pairing_for(&device_id).await?;
        if pairing.IsPaired().unwrap_or(false) {
            let level = pairing
                .ProtectionLevel()
                .unwrap_or(DevicePairingProtectionLevel::Default);
            // MeshCore requires an authenticated (PIN) bond. A Just Works bond
            // from another tool encrypts but the firmware still rejects it.
            if level != DevicePairingProtectionLevel::None
                && level != DevicePairingProtectionLevel::Encryption
            {
                return Ok(());
            }
            unpair(&pairing).await?;
            let pairing = pairing_for(&device_id).await?;
            return pair_with_pin(&pairing, pin).await;
        }
        pair_with_pin(&pairing, pin).await
    }

    async fn pairing_for(device_id: &HSTRING) -> Result<DeviceInformationPairing, String> {
        DeviceInformation::CreateFromIdAsync(device_id)
            .map_err(|err| format!("Bluetooth device info failed: {err}"))?
            .await
            .and_then(|info| info.Pairing())
            .map_err(|err| format!("Bluetooth pairing info unavailable: {err}"))
    }

    async fn unpair(pairing: &DeviceInformationPairing) -> Result<(), String> {
        let status = pairing
            .UnpairAsync()
            .map_err(|err| format!("Could not remove the old Bluetooth bond: {err}"))?
            .await
            .and_then(|result| result.Status())
            .map_err(|err| format!("Could not remove the old Bluetooth bond: {err}"))?;
        if status == DeviceUnpairingResultStatus::Unpaired
            || status == DeviceUnpairingResultStatus::AlreadyUnpaired
        {
            Ok(())
        } else {
            Err(format!(
                "Windows kept an old Bluetooth bond without PIN protection (status {}). Remove the radio in Settings → Bluetooth & devices, then Pair again.",
                status.0
            ))
        }
    }

    async fn pair_with_pin(pairing: &DeviceInformationPairing, pin: u32) -> Result<(), String> {
        let custom: DeviceInformationCustomPairing = pairing
            .Custom()
            .map_err(|err| format!("Windows custom pairing unavailable: {err}"))?;
        let token = register_pin_handler(&custom, pin)
            .map_err(|err| format!("Windows pairing handler failed: {err}"))?;
        let operation = custom.PairWithProtectionLevelAsync(
            DevicePairingKinds::ProvidePin
                | DevicePairingKinds::ConfirmOnly
                | DevicePairingKinds::ConfirmPinMatch,
            DevicePairingProtectionLevel::EncryptionAndAuthentication,
        );
        let outcome = match operation {
            Ok(operation) => tokio::time::timeout(PAIR_TIMEOUT, operation).await,
            Err(err) => {
                let _ = custom.RemovePairingRequested(token);
                return Err(format!("Windows pairing failed to start: {err}"));
            }
        };
        let _ = custom.RemovePairingRequested(token);
        let status = match outcome {
            Err(_) => {
                return Err(
                    "Windows pairing timed out. Power-cycle the radio, close the phone app, then Pair again."
                        .into(),
                )
            }
            Ok(result) => result
                .and_then(|result| result.Status())
                .map_err(|err| format!("Windows pairing failed: {err}"))?,
        };
        pair_status_result(status)
    }

    /// Answer the pairing ceremony with the operator's PIN. Kept synchronous:
    /// the handler object is not `Send`, and the registration holds its own
    /// reference, so it must not live across an `.await`.
    fn register_pin_handler(
        custom: &DeviceInformationCustomPairing,
        pin: u32,
    ) -> windows::core::Result<i64> {
        let pin_text = HSTRING::from(format!("{pin:06}"));
        let handler = TypedEventHandler::<DeviceInformationCustomPairing, DevicePairingRequestedEventArgs>::new(
            move |_, args: Ref<DevicePairingRequestedEventArgs>| {
                let args = args.ok()?;
                let kind = args.PairingKind()?;
                if kind == DevicePairingKinds::ProvidePin {
                    args.AcceptWithPin(&pin_text)
                } else if kind == DevicePairingKinds::ConfirmOnly
                    || kind == DevicePairingKinds::ConfirmPinMatch
                {
                    args.Accept()
                } else {
                    // Not accepting fails the ceremony with RejectedByHandler.
                    Ok(())
                }
            },
        );
        custom.PairingRequested(&handler)
    }

    fn pair_status_result(status: DevicePairingResultStatus) -> Result<(), String> {
        if status == DevicePairingResultStatus::Paired
            || status == DevicePairingResultStatus::AlreadyPaired
        {
            return Ok(());
        }
        let detail = if status == DevicePairingResultStatus::AuthenticationFailure
            || status == DevicePairingResultStatus::AuthenticationTimeout
        {
            "Wrong Bluetooth PIN or the radio rejected it. Check the six-digit PIN shown on the radio (screenless default 123456) and Pair again."
        } else if status == DevicePairingResultStatus::RejectedByHandler {
            "Windows asked for a pairing method MeshCore does not use. Remove the radio in Settings → Bluetooth & devices, power-cycle it, then Pair again."
        } else if status == DevicePairingResultStatus::ProtectionLevelCouldNotBeMet {
            "The radio did not offer PIN-protected pairing. Update the companion firmware, then Pair again."
        } else if status == DevicePairingResultStatus::ConnectionRejected
            || status == DevicePairingResultStatus::TooManyConnections
            || status == DevicePairingResultStatus::RemoteDeviceHasAssociation
        {
            "The radio refused the connection. Disconnect the phone MeshCore app (or other client), wait ~10 seconds, then Pair again."
        } else if status == DevicePairingResultStatus::OperationAlreadyInProgress {
            "Windows is already pairing this radio. Wait a few seconds, then try again."
        } else if status == DevicePairingResultStatus::PairingCanceled {
            "Pairing was cancelled. Pair again and keep the radio nearby."
        } else {
            "Windows could not pair the radio. Remove it in Settings → Bluetooth & devices, power-cycle the radio, then Pair again."
        };
        Err(format!("{detail} (Windows pairing status {})", status.0))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_ble_address;
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

    #[test]
    fn parses_ble_addresses_msb_first() {
        assert_eq!(parse_ble_address("AA:BB:CC:DD:EE:FF"), Some(0xAABB_CCDD_EEFF));
        assert_eq!(parse_ble_address(" aa-bb-cc-dd-ee-01 "), Some(0xAABB_CCDD_EE01));
        assert_eq!(parse_ble_address("aabbccddee02"), Some(0xAABB_CCDD_EE02));
    }

    #[test]
    fn rejects_names_and_malformed_addresses() {
        assert_eq!(parse_ble_address("MeshCore-1234"), None);
        assert_eq!(parse_ble_address("AA:BB:CC:DD:EE"), None);
        assert_eq!(parse_ble_address("AAA:B:CC:DD:EE:FF"), None);
        assert_eq!(parse_ble_address("GG:BB:CC:DD:EE:FF"), None);
    }
}
