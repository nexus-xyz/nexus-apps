//! How Gateway trusts a person's devices, and which parts of the Devices page show for it.
//!
//! `GET /v1/devices` names it in a top-level `trust`. With `approvals`, or with no field, a new device waits until a
//! trusted one approves it, and the page shows pairing as before. With `gateway`, Gateway vouches for every device the
//! person signs in on: a computer registers itself at sign-in, and the page shows no codes, QR codes or approvals.

use serde_json::Value;

/// How Gateway trusts this person's devices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DeviceTrust {
    /// A new device waits for approval from a trusted one.
    #[default]
    Approvals,
    /// Gateway trusts every device the person signs in on.
    Gateway,
}

/// The mode in `GET /v1/devices`'s answer. An absent or unknown value reads as approvals, so nothing changes for it.
pub(super) fn read_device_trust(value: &Value) -> DeviceTrust {
    match value.get("trust").and_then(Value::as_str) {
        Some("gateway") => DeviceTrust::Gateway,
        _ => DeviceTrust::Approvals,
    }
}

/// The parts of the Devices page that show. The devices list and Remote control show in both modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Parts {
    /// This device's pairing: its code and QR code, Request approval, Start again, the waiting card, and confirming
    /// the approver's code.
    pub pairing: bool,
    /// Add a device: finding a pending device by its typed code and approving it.
    pub add_device: bool,
    /// The phone connection: Show connection code, its QR code, and approving the device that opened it.
    pub connection_code: bool,
    /// Other computers, joined with a connection link.
    pub remote_computers: bool,
}

/// `preview`: whether the remote pairing preview is on, which shows Other computers in approvals mode.
pub(super) fn parts(trust: DeviceTrust, preview: bool) -> Parts {
    match trust {
        DeviceTrust::Approvals => Parts {
            pairing: true,
            add_device: true,
            connection_code: true,
            remote_computers: preview,
        },
        DeviceTrust::Gateway => Parts {
            pairing: false,
            add_device: false,
            connection_code: false,
            remote_computers: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceTrust, Parts, parts, read_device_trust};
    use serde_json::json;

    #[test]
    fn reads_the_mode_and_takes_anything_else_as_approvals() {
        assert_eq!(
            read_device_trust(&json!({ "trust": "gateway", "devices": [] })),
            DeviceTrust::Gateway
        );
        assert_eq!(
            read_device_trust(&json!({ "trust": "approvals", "devices": [] })),
            DeviceTrust::Approvals
        );
        for other in [
            json!({ "devices": [] }),
            json!({ "trust": null }),
            json!({ "trust": "Gateway" }),
            json!({ "trust": true }),
        ] {
            assert_eq!(read_device_trust(&other), DeviceTrust::Approvals, "{other}");
        }
    }

    #[test]
    fn approvals_mode_shows_pairing_as_before() {
        let shown = Parts {
            pairing: true,
            add_device: true,
            connection_code: true,
            remote_computers: false,
        };
        assert_eq!(parts(DeviceTrust::Approvals, false), shown);
        assert_eq!(
            parts(DeviceTrust::Approvals, true),
            Parts {
                remote_computers: true,
                ..shown
            }
        );
    }

    #[test]
    fn gateway_mode_hides_every_code_and_approval_and_shows_other_computers() {
        for preview in [false, true] {
            assert_eq!(
                parts(DeviceTrust::Gateway, preview),
                Parts {
                    pairing: false,
                    add_device: false,
                    connection_code: false,
                    remote_computers: true,
                }
            );
        }
    }
}
