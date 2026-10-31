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

/// `#` is a future bot prefix. `signup` and `status` are the enrollment desk.
pub fn parse_command(text: &str) -> InboundCommand {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == "status" {
        return InboundCommand::Status;
    }
    if lower == "signup" {
        return InboundCommand::Signup { name: None };
    }
    if let Some(rest) = lower.strip_prefix("signup ") {
        let raw = trimmed[trimmed.len() - rest.len()..].trim();
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
        InboundCommand::Other => {
            if current == Enrollment::Enrolled || current == Enrollment::Hq {
                ReplyDecision {
                    enrollment: current,
                    reply: None,
                    requested_name: None,
                }
            } else {
                ReplyDecision {
                    enrollment: current,
                    reply: Some("Send signup to enroll.".into()),
                    requested_name: None,
                }
            }
        }
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

pub fn approval_message(channel: &str, secret_hex: &str) -> String {
    fit_mesh(&format!(
        "Approved. Add private channel {channel}, secret {secret_hex}."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_signup_forms() {
        assert_eq!(
            parse_command("  SIGNUP  "),
            InboundCommand::Signup { name: None }
        );
        assert_eq!(
            parse_command("signup Ada Lovelace"),
            InboundCommand::Signup {
                name: Some("Ada Lovelace".into())
            }
        );
        assert_eq!(parse_command("status"), InboundCommand::Status);
        assert_eq!(parse_command("hello"), InboundCommand::Other);
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
    fn approval_text_fits_one_frame() {
        let text = approval_message("METIS-INFO", &"ab".repeat(16));
        assert!(text.len() <= 160);
        assert!(text.contains("secret"));
    }
}
