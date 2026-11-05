//! Direct-message signup parsing. Mesh text is untrusted and never executed.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enrollment {
    Heard,
    Pending,
    Enrolled,
    Denied,
    Hq,
}

impl Enrollment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heard => "heard",
            Self::Pending => "pending",
            Self::Enrolled => "enrolled",
            Self::Denied => "denied",
            Self::Hq => "hq",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "pending" => Self::Pending,
            "enrolled" => Self::Enrolled,
            "denied" => Self::Denied,
            "hq" => Self::Hq,
            _ => Self::Heard,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Heard => "Heard",
            Self::Pending => "Pending",
            Self::Enrolled => "Enrolled",
            Self::Denied => "Denied",
            Self::Hq => "Headquarters",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundCommand {
    Signup { name: Option<String> },
    Status,
    Other,
}

/// Enrollment desk DMs use the same `#` command marker as channel bots.
/// Bare `signup` / `status` are ordinary chat and do not trigger a reply.
pub fn parse_command(text: &str) -> InboundCommand {
    let trimmed = text.trim();
    let Some(bare) = trimmed.strip_prefix('#') else {
        return InboundCommand::Other;
    };
    let bare = bare.trim();
    let lower = bare.to_ascii_lowercase();
    if lower == "status" {
        return InboundCommand::Status;
    }
    if lower == "signup" {
        return InboundCommand::Signup { name: None };
    }
    if let Some(rest) = lower.strip_prefix("signup ") {
        let raw = bare[bare.len() - rest.len()..].trim();
        let name = if raw.is_empty() {
            None
        } else {
            Some(raw.chars().take(32).collect())
        };
        return InboundCommand::Signup { name };
    }
    InboundCommand::Other
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyDecision {
    pub enrollment: Enrollment,
    pub reply: Option<String>,
    pub requested_name: Option<String>,
}

/// Decide the enrollment change and the short reply.
///
/// `unique_contact` is false when the 6-byte prefix matches zero or several keys.
/// Ambiguous and unknown senders get no automatic transmit.
pub fn decide(command: &InboundCommand, current: Enrollment, unique_contact: bool) -> ReplyDecision {
    if !unique_contact {
        return ReplyDecision {
            enrollment: current,
            reply: None,
            requested_name: None,
        };
    }

    match command {
        InboundCommand::Signup { name } => {
            if current == Enrollment::Enrolled || current == Enrollment::Hq {
                return ReplyDecision {
                    enrollment: current,
                    reply: Some("You are enrolled.".into()),
                    requested_name: None,
                };
            }
            ReplyDecision {
                enrollment: Enrollment::Pending,
                reply: Some("Pending. Ask an operator to approve.".into()),
                requested_name: name.clone(),
            }
        }
        InboundCommand::Status => ReplyDecision {
            enrollment: current,
            reply: Some(status_line(current).into()),
            requested_name: None,
        },
        // Casual DMs stay silent — enrollment desk is #signup / #status only.
        InboundCommand::Other => ReplyDecision {
            enrollment: current,
            reply: None,
            requested_name: None,
        },
    }
}

fn status_line(current: Enrollment) -> &'static str {
    match current {
        Enrollment::Pending => "pending",
        Enrollment::Enrolled | Enrollment::Hq => "approved",
        Enrollment::Heard | Enrollment::Denied => "not enrolled",
    }
}

/// MeshCore text payloads are at most 160 bytes.
pub fn fit_mesh(text: &str) -> String {
    if text.len() <= 160 {
        return text.to_string();
    }
    let mut end = 160;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Split long plain text into ≤160-byte mesh frames.
///
/// When more than one frame is needed, each line is prefixed `tag i/n: `.
/// Prefer breaking on whitespace so vehicle/person lines stay readable.
pub fn mesh_frames(tag: &str, body: &str, max_frames: usize) -> Vec<String> {
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if body.is_empty() {
        return Vec::new();
    }
    let max_frames = max_frames.max(1).min(6);
    // Reserve room for "TAG 9/9: " (worst case ~12 bytes + tag).
    let overhead = tag.len() + 8;
    let chunk_budget = 160usize.saturating_sub(overhead).max(40);

    let mut pieces: Vec<String> = Vec::new();
    let mut rest = body.as_str();
    while !rest.is_empty() && pieces.len() < max_frames {
        if rest.len() <= chunk_budget {
            pieces.push(rest.to_string());
            rest = "";
            break;
        }
        let mut cut = chunk_budget.min(rest.len());
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        // Prefer last space in the window.
        if let Some(rel) = rest[..cut].rfind(' ') {
            if rel >= chunk_budget / 3 {
                cut = rel;
            }
        }
        let (take, leave) = rest.split_at(cut);
        pieces.push(take.trim().to_string());
        rest = leave.trim_start();
    }
    if pieces.is_empty() {
        return Vec::new();
    }
    if !rest.is_empty() {
        // Last frame was truncated — mark it.
        if let Some(last) = pieces.last_mut() {
            *last = fit_mesh(&format!("{last}…"));
        }
    }
    if pieces.len() == 1 {
        let line = fit_mesh(&format!("{tag}: {}", pieces[0]));
        return vec![line];
    }
    let n = pieces.len();
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, part)| fit_mesh(&format!("{tag} {}/{n}: {part}", i + 1)))
        .collect()
}

pub fn approval_message(channel: &str, secret_hex: &str) -> String {
    fit_mesh(&format!(
        "Approved. Add private channel {channel}, secret {secret_hex}."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_frames_split_long_body() {
        let body = "Suspect vehicle: blue 2018 Honda Civic CA plate 7ABC123. \
            Missing child: Jane Doe, age 7, brown hair, red jacket. \
            Last seen near I-15 and Main St Hesperia around 10:15 AM. \
            Suspect: adult male, approx 35, dark hoodie. Call 911.";
        let frames = mesh_frames("AMBER+", body, 4);
        assert!(frames.len() >= 2, "{frames:?}");
        assert!(frames.iter().all(|f| f.len() <= 160));
        assert!(frames[0].contains("1/") || frames[0].starts_with("AMBER+:"));
    }

    #[test]
    fn parses_signup_forms() {
        assert_eq!(
            parse_command("  #SIGNUP  "),
            InboundCommand::Signup { name: None }
        );
        assert_eq!(
            parse_command("#signup Ada Lovelace"),
            InboundCommand::Signup {
                name: Some("Ada Lovelace".into())
            }
        );
        assert_eq!(parse_command("#status"), InboundCommand::Status);
        assert_eq!(parse_command("hello"), InboundCommand::Other);
        // Bare words are chat, not commands.
        assert_eq!(parse_command("signup"), InboundCommand::Other);
        assert_eq!(parse_command("status"), InboundCommand::Other);
    }

    #[test]
    fn ambiguous_prefix_does_not_reply() {
        let decision = decide(
            &InboundCommand::Signup { name: None },
            Enrollment::Heard,
            false,
        );
        assert!(decision.reply.is_none());
        assert_eq!(decision.enrollment, Enrollment::Heard);
    }

    #[test]
    fn signup_moves_denied_back_to_pending() {
        let decision = decide(
            &InboundCommand::Signup {
                name: Some("Ada".into()),
            },
            Enrollment::Denied,
            true,
        );
        assert_eq!(decision.enrollment, Enrollment::Pending);
        assert_eq!(
            decision.reply.as_deref(),
            Some("Pending. Ask an operator to approve.")
        );
        assert_eq!(decision.requested_name.as_deref(), Some("Ada"));
    }

    #[test]
    fn enrolled_node_is_not_asked_to_signup_again() {
        let decision = decide(&InboundCommand::Other, Enrollment::Enrolled, true);
        assert!(decision.reply.is_none());
        let status = decide(&InboundCommand::Status, Enrollment::Enrolled, true);
        assert_eq!(status.reply.as_deref(), Some("approved"));
    }

    #[test]
    fn casual_dm_from_heard_does_not_nag_signup() {
        let decision = decide(&InboundCommand::Other, Enrollment::Heard, true);
        assert!(decision.reply.is_none());
        let denied = decide(&InboundCommand::Other, Enrollment::Denied, true);
        assert!(denied.reply.is_none());
        let pending = decide(&InboundCommand::Other, Enrollment::Pending, true);
        assert!(pending.reply.is_none());
    }

    #[test]
    fn approval_text_fits_one_frame() {
        let text = approval_message("METIS-INFO", &"ab".repeat(16));
        assert!(text.len() <= 160);
        assert!(text.contains("secret"));
    }
}
