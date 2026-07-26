//! Visual tokens: the colours the views paint with.

use egui::Color32;

use vmiscope_core::{Protocol, Risk};

/// Colour for a subscription risk level.
pub(crate) fn risk_color(risk: Risk) -> Color32 {
    match risk {
        Risk::High => Color32::from_rgb(240, 100, 100),
        Risk::Medium => Color32::from_rgb(225, 185, 90),
        Risk::Low => Color32::from_rgb(150, 165, 150),
    }
}

/// Colour a connection by protocol/state (before fade alpha is applied).
pub(crate) fn state_color(state: &str, proto: Protocol) -> Color32 {
    if proto == Protocol::Udp {
        return Color32::from_rgb(120, 170, 220);
    }
    match state {
        "Established" => Color32::from_rgb(120, 210, 140), // green — active
        "Listen" | "Bound" => Color32::from_rgb(120, 170, 220), // blue — server socket
        "SynSent" | "SynReceived" | "FinWait1" | "FinWait2" | "CloseWait" | "Closing"
        | "LastAck" | "TimeWait" => Color32::from_rgb(220, 190, 110), // amber — transitional
        "Closed" | "DeleteTCB" => Color32::from_rgb(165, 165, 165), // gray
        _ => Color32::from_rgb(205, 205, 205),
    }
}
