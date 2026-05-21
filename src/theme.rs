use std::{env, fs, io, path::PathBuf};

use ratatui::style::Color;

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) name: &'static str,
    pub(crate) background: Color,
    pub(crate) foreground: Color,
    pub(crate) muted: Color,
    pub(crate) accent: Color,
    /// Background color used for pane title bars. Slightly distinct from
    /// `background` so the title bar reads as a discrete UI element.
    pub(crate) title_bar: Color,
    /// When true, terminal content should not be recolored by the app.
    pub(crate) passthrough: bool,
    /// Common 16-color terminal palette for this theme, in ANSI order.
    pub(crate) palette: [Color; 16],
}

pub(crate) const THEMES: [Theme; 9] = [
    Theme {
        name: "None",
        background: Color::Reset,
        foreground: Color::Reset,
        muted: Color::Reset,
        accent: Color::Reset,
        title_bar: Color::Reset,
        passthrough: true,
        palette: [
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
            Color::Reset,
        ],
    },
    Theme {
        name: "Classic",
        background: Color::Black,
        foreground: Color::White,
        muted: Color::Gray,
        accent: Color::White,
        title_bar: Color::DarkGray,
        passthrough: false,
        palette: [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ],
    },
    Theme {
        name: "Dracula",
        background: Color::Rgb(40, 42, 54),
        foreground: Color::Rgb(248, 248, 242),
        muted: Color::Rgb(108, 113, 196),
        accent: Color::Rgb(255, 121, 198),
        title_bar: Color::Rgb(68, 71, 90),
        passthrough: false,
        palette: [
            Color::Rgb(40, 42, 54),
            Color::Rgb(255, 85, 85),
            Color::Rgb(80, 250, 123),
            Color::Rgb(241, 250, 140),
            Color::Rgb(98, 114, 164),
            Color::Rgb(255, 121, 198),
            Color::Rgb(139, 233, 253),
            Color::Rgb(248, 248, 242),
            Color::Rgb(68, 71, 90),
            Color::Rgb(255, 110, 110),
            Color::Rgb(189, 250, 107),
            Color::Rgb(255, 255, 167),
            Color::Rgb(141, 156, 227),
            Color::Rgb(255, 184, 228),
            Color::Rgb(167, 249, 255),
            Color::Rgb(255, 255, 255),
        ],
    },
    Theme {
        name: "One Dark",
        background: Color::Rgb(40, 44, 52),
        foreground: Color::Rgb(171, 178, 191),
        muted: Color::Rgb(92, 99, 112),
        accent: Color::Rgb(97, 175, 239),
        title_bar: Color::Rgb(61, 66, 77),
        passthrough: false,
        palette: [
            Color::Rgb(40, 44, 52),
            Color::Rgb(224, 108, 117),
            Color::Rgb(152, 195, 121),
            Color::Rgb(229, 192, 123),
            Color::Rgb(97, 175, 239),
            Color::Rgb(198, 120, 221),
            Color::Rgb(86, 182, 194),
            Color::Rgb(171, 178, 191),
            Color::Rgb(92, 99, 112),
            Color::Rgb(255, 122, 138),
            Color::Rgb(171, 209, 136),
            Color::Rgb(209, 154, 102),
            Color::Rgb(97, 175, 239),
            Color::Rgb(210, 153, 255),
            Color::Rgb(86, 182, 194),
            Color::Rgb(255, 255, 255),
        ],
    },
    Theme {
        name: "Gruvbox Dark",
        background: Color::Rgb(40, 40, 40),
        foreground: Color::Rgb(235, 219, 178),
        muted: Color::Rgb(146, 131, 116),
        accent: Color::Rgb(250, 189, 47),
        title_bar: Color::Rgb(60, 56, 54),
        passthrough: false,
        palette: [
            Color::Rgb(40, 40, 40),
            Color::Rgb(204, 36, 29),
            Color::Rgb(152, 151, 26),
            Color::Rgb(215, 153, 33),
            Color::Rgb(69, 133, 136),
            Color::Rgb(177, 98, 134),
            Color::Rgb(104, 157, 106),
            Color::Rgb(235, 219, 178),
            Color::Rgb(146, 131, 116),
            Color::Rgb(251, 73, 52),
            Color::Rgb(184, 187, 38),
            Color::Rgb(250, 189, 47),
            Color::Rgb(131, 165, 152),
            Color::Rgb(211, 134, 155),
            Color::Rgb(142, 192, 124),
            Color::Rgb(251, 241, 199),
        ],
    },
    Theme {
        name: "Nord",
        background: Color::Rgb(46, 52, 64),
        foreground: Color::Rgb(216, 222, 233),
        muted: Color::Rgb(76, 86, 106),
        accent: Color::Rgb(136, 192, 208),
        title_bar: Color::Rgb(59, 66, 82),
        passthrough: false,
        palette: [
            Color::Rgb(46, 52, 64),
            Color::Rgb(191, 97, 106),
            Color::Rgb(163, 190, 140),
            Color::Rgb(235, 203, 139),
            Color::Rgb(129, 161, 193),
            Color::Rgb(180, 142, 173),
            Color::Rgb(136, 192, 208),
            Color::Rgb(216, 222, 233),
            Color::Rgb(76, 86, 106),
            Color::Rgb(208, 135, 112),
            Color::Rgb(163, 190, 140),
            Color::Rgb(235, 203, 139),
            Color::Rgb(129, 161, 193),
            Color::Rgb(180, 142, 173),
            Color::Rgb(143, 188, 187),
            Color::Rgb(236, 239, 244),
        ],
    },
    Theme {
        name: "Solarized Dark",
        background: Color::Rgb(0, 43, 54),
        foreground: Color::Rgb(238, 232, 213),
        muted: Color::Rgb(101, 123, 131),
        accent: Color::Rgb(38, 139, 210),
        title_bar: Color::Rgb(7, 54, 66),
        passthrough: false,
        palette: [
            Color::Rgb(0, 43, 54),
            Color::Rgb(220, 50, 47),
            Color::Rgb(133, 153, 0),
            Color::Rgb(181, 137, 0),
            Color::Rgb(38, 139, 210),
            Color::Rgb(211, 54, 130),
            Color::Rgb(42, 161, 152),
            Color::Rgb(238, 232, 213),
            Color::Rgb(7, 54, 66),
            Color::Rgb(203, 75, 22),
            Color::Rgb(88, 110, 117),
            Color::Rgb(101, 123, 131),
            Color::Rgb(131, 148, 150),
            Color::Rgb(108, 113, 196),
            Color::Rgb(147, 161, 161),
            Color::Rgb(253, 246, 227),
        ],
    },
    Theme {
        name: "Synthwave",
        background: Color::Rgb(22, 7, 43),
        foreground: Color::Rgb(255, 230, 250),
        muted: Color::Rgb(120, 96, 173),
        accent: Color::Rgb(255, 79, 222),
        title_bar: Color::Rgb(43, 16, 76),
        passthrough: false,
        palette: [
            Color::Rgb(22, 7, 43),
            Color::Rgb(255, 79, 133),
            Color::Rgb(0, 245, 212),
            Color::Rgb(249, 248, 113),
            Color::Rgb(106, 92, 255),
            Color::Rgb(255, 79, 222),
            Color::Rgb(79, 212, 255),
            Color::Rgb(255, 230, 250),
            Color::Rgb(43, 16, 76),
            Color::Rgb(255, 122, 166),
            Color::Rgb(85, 255, 180),
            Color::Rgb(255, 240, 130),
            Color::Rgb(156, 140, 255),
            Color::Rgb(255, 140, 234),
            Color::Rgb(140, 255, 242),
            Color::Rgb(255, 255, 255),
        ],
    },
    Theme {
        name: "Light",
        background: Color::Rgb(245, 245, 240),
        foreground: Color::Rgb(32, 32, 32),
        muted: Color::Rgb(90, 90, 90),
        accent: Color::Rgb(32, 32, 32),
        title_bar: Color::Rgb(220, 220, 212),
        passthrough: false,
        palette: [
            Color::Rgb(245, 245, 240),
            Color::Rgb(198, 40, 40),
            Color::Rgb(46, 125, 50),
            Color::Rgb(245, 124, 0),
            Color::Rgb(21, 101, 192),
            Color::Rgb(123, 31, 162),
            Color::Rgb(0, 121, 107),
            Color::Rgb(48, 48, 48),
            Color::Rgb(120, 120, 120),
            Color::Rgb(229, 57, 53),
            Color::Rgb(67, 160, 71),
            Color::Rgb(251, 192, 45),
            Color::Rgb(66, 165, 245),
            Color::Rgb(171, 71, 188),
            Color::Rgb(38, 166, 154),
            Color::Rgb(17, 17, 17),
        ],
    },
];

fn theme_persistence_path() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("split_tui")
            .join("theme"),
    )
}

pub(crate) fn load_persisted_theme_index() -> Option<usize> {
    let path = theme_persistence_path()?;
    let name = fs::read_to_string(path).ok()?.trim().to_string();
    THEMES.iter().position(|theme| theme.name == name)
}

pub(crate) fn default_theme_index() -> usize {
    THEMES
        .iter()
        .position(|theme| !theme.passthrough)
        .unwrap_or(0)
}

pub(crate) fn save_persisted_theme(theme: Theme) -> io::Result<()> {
    let Some(path) = theme_persistence_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, format!("{}\n", theme.name))
}
