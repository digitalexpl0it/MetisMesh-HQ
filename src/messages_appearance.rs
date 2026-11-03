//! Local Messages pane appearance (global + per-channel). Never sent over mesh.

use serde::{Deserialize, Serialize};

pub const APPEARANCE_SETTING: &str = "messages_appearance";
pub const BG_OPACITY_MAX: u8 = 40;
pub const BG_OPACITY_DEFAULT: u8 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BgMode {
    #[default]
    Solid,
    Gradient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BgAngle {
    #[default]
    Vertical,
    Diagonal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    Sm,
    #[default]
    Md,
    Lg,
}

impl TextSize {
    pub fn css_px(self) -> &'static str {
        match self {
            Self::Sm => "0.8125rem",
            Self::Md => "0.875rem",
            Self::Lg => "1rem",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagesAppearance {
    #[serde(default)]
    pub bg_mode: BgMode,
    #[serde(default = "default_bg_color")]
    pub bg_color: String,
    #[serde(default)]
    pub bg_color_b: Option<String>,
    #[serde(default)]
    pub bg_angle: BgAngle,
    #[serde(default = "default_bg_opacity")]
    pub bg_opacity: u8,
    #[serde(default)]
    pub text_size: TextSize,
    #[serde(default)]
    pub text_color: Option<String>,
}

fn default_bg_color() -> String {
    "#101418".into()
}

fn default_bg_opacity() -> u8 {
    BG_OPACITY_DEFAULT
}

impl Default for MessagesAppearance {
    fn default() -> Self {
        Self {
            bg_mode: BgMode::Solid,
            bg_color: default_bg_color(),
            bg_color_b: Some("#16324f".into()),
            bg_angle: BgAngle::Vertical,
            bg_opacity: BG_OPACITY_DEFAULT,
            text_size: TextSize::Md,
            text_color: None,
        }
    }
}

impl MessagesAppearance {
    pub fn clamp(mut self) -> Self {
        self.bg_opacity = self.bg_opacity.min(BG_OPACITY_MAX);
        self.bg_color = normalize_hex(&self.bg_color).unwrap_or_else(default_bg_color);
        if let Some(b) = self.bg_color_b.take() {
            self.bg_color_b = normalize_hex(&b);
        }
        if self.bg_color_b.is_none() {
            self.bg_color_b = Some("#16324f".into());
        }
        if let Some(c) = self.text_color.take() {
            self.text_color = normalize_hex(&c);
        }
        self
    }

    pub fn themed(&self) -> bool {
        self.bg_opacity > 0
            || self.text_size != TextSize::Md
            || self.text_color.is_some()
            || self.bg_mode == BgMode::Gradient
    }

    /// Inline style string for the messages scroller/pane.
    pub fn pane_style(&self) -> String {
        let a = self.bg_opacity as f32 / 100.0;
        let color_a = with_alpha(&self.bg_color, a);
        let color_b = with_alpha(
            self.bg_color_b.as_deref().unwrap_or("#16324f"),
            a,
        );
        let bg = match self.bg_mode {
            BgMode::Solid => format!("background: {color_a};"),
            BgMode::Gradient => {
                let angle = match self.bg_angle {
                    BgAngle::Vertical => "180deg",
                    BgAngle::Diagonal => "135deg",
                };
                format!("background: linear-gradient({angle}, {color_a}, {color_b});")
            }
        };
        let size = self.text_size.css_px();
        let text = self
            .text_color
            .as_deref()
            .map(|c| format!("--msg-text-color: {c}; color: {c};"))
            .unwrap_or_default();
        format!(
            "{bg} --msg-pane-opacity: {a}; --msg-text-size: {size}; font-size: {size}; {text}"
        )
    }
}

pub fn normalize_hex(raw: &str) -> Option<String> {
    let s = raw.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 3 && s.len() != 6 {
        return None;
    }
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let expanded = if s.len() == 3 {
        s.chars()
            .flat_map(|c| [c, c])
            .collect::<String>()
            .to_ascii_lowercase()
    } else {
        s.to_ascii_lowercase()
    };
    Some(format!("#{expanded}"))
}

fn with_alpha(hex: &str, alpha: f32) -> String {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return format!("rgba(16, 20, 24, {alpha})");
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(16);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(20);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(24);
    format!("rgba({r}, {g}, {b}, {alpha})")
}

pub fn parse_appearance_json(raw: &str) -> MessagesAppearance {
    serde_json::from_str::<MessagesAppearance>(raw)
        .unwrap_or_default()
        .clamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_opacity_and_hex() {
        let a = MessagesAppearance {
            bg_opacity: 99,
            bg_color: "abc".into(),
            text_color: Some("#ff00".into()),
            ..Default::default()
        }
        .clamp();
        assert_eq!(a.bg_opacity, BG_OPACITY_MAX);
        assert_eq!(a.bg_color, "#aabbcc");
        assert!(a.text_color.is_none());
    }

    #[test]
    fn pane_style_includes_gradient() {
        let a = MessagesAppearance {
            bg_mode: BgMode::Gradient,
            bg_opacity: 20,
            ..Default::default()
        }
        .clamp();
        let style = a.pane_style();
        assert!(style.contains("linear-gradient"));
        assert!(style.contains("--msg-text-size"));
    }
}
