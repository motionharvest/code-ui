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
}

pub(crate) const THEMES: [Theme; 7] = [
    Theme {
        name: "Classic",
        background: Color::Black,
        foreground: Color::White,
        muted: Color::Gray,
        accent: Color::White,
        title_bar: Color::DarkGray,
    },
    Theme {
        name: "Dracula",
        background: Color::Rgb(40, 42, 54),
        foreground: Color::Rgb(248, 248, 242),
        muted: Color::Rgb(108, 113, 196),
        accent: Color::Rgb(255, 121, 198),
        title_bar: Color::Rgb(68, 71, 90),
    },
    Theme {
        name: "One Dark",
        background: Color::Rgb(40, 44, 52),
        foreground: Color::Rgb(171, 178, 191),
        muted: Color::Rgb(92, 99, 112),
        accent: Color::Rgb(97, 175, 239),
        title_bar: Color::Rgb(61, 66, 77),
    },
    Theme {
        name: "Gruvbox Dark",
        background: Color::Rgb(40, 40, 40),
        foreground: Color::Rgb(235, 219, 178),
        muted: Color::Rgb(146, 131, 116),
        accent: Color::Rgb(250, 189, 47),
        title_bar: Color::Rgb(60, 56, 54),
    },
    Theme {
        name: "Nord",
        background: Color::Rgb(46, 52, 64),
        foreground: Color::Rgb(216, 222, 233),
        muted: Color::Rgb(76, 86, 106),
        accent: Color::Rgb(136, 192, 208),
        title_bar: Color::Rgb(59, 66, 82),
    },
    Theme {
        name: "Solarized Dark",
        background: Color::Rgb(0, 43, 54),
        foreground: Color::Rgb(238, 232, 213),
        muted: Color::Rgb(101, 123, 131),
        accent: Color::Rgb(38, 139, 210),
        title_bar: Color::Rgb(7, 54, 66),
    },
    Theme {
        name: "Light",
        background: Color::Rgb(245, 245, 240),
        foreground: Color::Rgb(32, 32, 32),
        muted: Color::Rgb(90, 90, 90),
        accent: Color::Rgb(32, 32, 32),
        title_bar: Color::Rgb(220, 220, 212),
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

pub(crate) fn save_persisted_theme(theme: Theme) -> io::Result<()> {
    let Some(path) = theme_persistence_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, format!("{}\n", theme.name))
}
