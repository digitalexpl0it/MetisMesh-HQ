//! Small stroke icons. They inherit the current text color.

use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    Map,
    Users,
    Inbox,
    Messages,
    Settings,
    /// Funnel — Map filter menu.
    Filter,
    Bluetooth,
    Usb,
    Scan,
    Unlink,
    Check,
    Close,
    Ban,
    Trash,
    Radio,
    Key,
    Clock,
    Signal,
    Locate,
    Track,
    Dashboard,
    Menu,
    PanelClose,
    ChevronRight,
    /// Down chevron — expanded rail section.
    ChevronDown,
    /// Outline star — favorite / pin.
    Star,
    Reply,
    Quote,
    Copy,
    Emoji,
    Weather,
    /// Terminal window with prompt — Device CLI.
    Terminal,
    /// Circle with ? — Help.
    Help,
    /// Circle with i — About.
    Info,
    /// Plus — compose attach / share menu.
    Plus,
    /// Open folder — reveal files in the system file manager.
    Folder,
    /// Megaphone — Broadcast page.
    Broadcast,
    /// Pencil — edit row / group.
    Pencil,
}

#[component]
pub fn Icon(name: IconName, #[props(default = 16)] size: u32) -> Element {
    let paths = icon_paths(name);
    rsx! {
        svg {
            class: "shrink-0",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.75",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            for d in paths {
                path { d: "{d}" }
            }
        }
    }
}

fn icon_paths(name: IconName) -> &'static [&'static str] {
    match name {
        IconName::Map => &[
            "M9 18l-6 3V6l6-3 6 3 6-3v15l-6 3-6-3z",
            "M9 3v15",
            "M15 6v15",
        ],
        IconName::Users => &[
            "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2",
            "M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
            "M22 21v-2a4 4 0 0 0-3-3.87",
            "M16 3.13a4 4 0 0 1 0 7.75",
        ],
        IconName::Inbox => &[
            "M22 12h-6l-2 3h-4l-2-3H2",
            "M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z",
        ],
        IconName::Messages => &["M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"],
        IconName::Settings => &[
            "M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z",
            "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.604.852.997 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z",
        ],
        IconName::Filter => &[
            "M22 3H2l8 9.46V19l4 2v-8.54L22 3z",
        ],
        IconName::Bluetooth => &["m7 7 10 10-5 5V2l5 5L7 17"],
        IconName::Usb => &[
            "M4 10h16",
            "M8 6v4",
            "M16 6v4",
            "M6 10v4a6 6 0 0 0 12 0v-4",
            "M12 18v2",
        ],
        IconName::Scan => &[
            "M3 7V5a2 2 0 0 1 2-2h2",
            "M17 3h2a2 2 0 0 1 2 2v2",
            "M21 17v2a2 2 0 0 1-2 2h-2",
            "M7 21H5a2 2 0 0 1-2-2v-2",
            "M7 12h10",
        ],
        IconName::Unlink => &[
            "m18.84 12.25 1.72-1.71a5 5 0 0 0-7.07-7.07l-1.71 1.71",
            "m5.17 11.75-1.71 1.71a5 5 0 0 0 7.07 7.07l1.71-1.71",
            "m8 2 1 4",
            "m2 8 4 1",
            "m16 22-1-4",
            "m22 16-4-1",
        ],
        IconName::Check => &["M20 6 9 17l-5-5"],
        IconName::Close => &["M18 6 6 18", "M6 6l12 12"],
        IconName::Ban => &["M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z", "m4.9 4.9 14.2 14.2"],
        IconName::Trash => &[
            "M3 6h18",
            "M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2",
            "M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6",
            "M10 11v6",
            "M14 11v6",
        ],
        IconName::Radio => &[
            "M4.9 19.1a10 10 0 0 1 0-14.2",
            "M7.8 16.2a6 6 0 0 1 0-8.4",
            "M12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2z",
            "M16.2 16.2a6 6 0 0 0 0-8.4",
            "M19.1 19.1a10 10 0 0 0 0-14.2",
        ],
        IconName::Key => &[
            "M21 2l-2 2",
            "m15.5 7.5 2.3 2.3a1 1 0 0 1 0 1.4l-2.6 2.6a1 1 0 0 1-1.4 0L11.5 11",
            "M2 22l6-6",
            "M7.5 21.5 2-2",
            "M11 13a5 5 0 1 1-7-7 5 5 0 0 1 7 7z",
        ],
        IconName::Clock => &[
            "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z",
            "M12 6v6l4 2",
        ],
        IconName::Signal => &[
            "M2 20h2",
            "M7 20v-4",
            "M12 20V10",
            "M17 20V6",
            "M22 20V2",
        ],
        IconName::Locate => &[
            "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z",
            "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
            "M12 2v2",
            "M12 20v2",
            "M2 12h2",
            "M20 12h2",
        ],
        // Route / breadcrumb — Tracker whitelist movers.
        IconName::Track => &[
            "M4 15l4-8 4 4 4-8 4 8",
            "M4 19h16",
        ],
        IconName::Dashboard => &[
            "M3 3h8v8H3z",
            "M13 3h8v5h-8z",
            "M13 10h8v11h-8z",
            "M3 13h8v8H3z",
        ],
        IconName::Menu => &["M4 6h16", "M4 12h16", "M4 18h16"],
        IconName::PanelClose => &[
            "M4 4h16v16H4z",
            "M9 4v16",
            "M17 10l-3 2 3 2",
        ],
        IconName::ChevronRight => &["m9 18 6-6-6-6"],
        IconName::ChevronDown => &["m6 9 6 6 6-6"],
        IconName::Star => &[
            "M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z",
        ],
        IconName::Reply => &[
            "M9 17H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11",
            "m15 17 5-5-5-5",
            "M4 7h11a4 4 0 0 1 4 4v1",
        ],
        IconName::Quote => &[
            "M3 21c3 0 7-1 7-8V5c0-1.25-.756-2.017-2-2H4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2 1 0 1 0 1 1v1c0 1-1 2-2 2s-1 .008-1 1.031V21z",
            "M15 21c3 0 7-1 7-8V5c0-1.25-.757-2.017-2-2h-4c-1.25 0-2 .75-2 1.972V11c0 1.25.75 2 2 2h.75c0 2.25.25 4-2.75 4v3z",
        ],
        IconName::Copy => &[
            "M8 8H6a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-2",
            "M16 2H10a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V6z",
        ],
        IconName::Emoji => &[
            "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z",
            "M8 14s1.5 2 4 2 4-2 4-2",
            "M9 9h.01",
            "M15 9h.01",
        ],
        // Sun / cloud glance for the Weather page.
        IconName::Weather => &[
            "M12 3v2",
            "M12 19v2",
            "M5.6 5.6l1.4 1.4",
            "M17 17l1.4 1.4",
            "M3 12h2",
            "M19 12h2",
            "M5.6 18.4 7 17",
            "M17 7l1.4-1.4",
            "M16 14a4 4 0 1 0-7.5-1.5A3.5 3.5 0 1 0 9 18h7a3 3 0 0 0 0-6z",
        ],
        // Terminal — CLI prompt inside a window.
        IconName::Terminal => &[
            "M4 5a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5z",
            "m8 9 3 3-3 3",
            "M13 15h4",
        ],
        // Help — circle with ?.
        IconName::Help => &[
            "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z",
            "M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3",
            "M12 17h.01",
        ],
        // About — circle with i.
        IconName::Info => &[
            "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z",
            "M12 16v-4",
            "M12 8h.01",
        ],
        IconName::Plus => &["M12 5v14", "M5 12h14"],
        IconName::Folder => &[
            "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
        ],
        IconName::Broadcast => &[
            "M3 11v2a1 1 0 0 0 1 1h2l5 5V5L6 10H4a1 1 0 0 0-1 1z",
            "M15.54 8.46a5 5 0 0 1 0 7.07",
            "M19.07 4.93a10 10 0 0 1 0 14.14",
        ],
        IconName::Pencil => &[
            "M12 20h9",
            "M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z",
        ],
    }
}
