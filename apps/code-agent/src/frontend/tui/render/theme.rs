use crate::theme::{ThemePalette, active_palette};
use ratatui::style::Color;
use ratatui_core::style::{Color as CoreColor, Modifier as CoreModifier, Style as CoreStyle};
use tui_markdown::StyleSheet as MarkdownStyleSheet;

pub(super) fn palette() -> ThemePalette {
    active_palette()
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct NanoclawMarkdownStyleSheet;

impl MarkdownStyleSheet for NanoclawMarkdownStyleSheet {
    fn heading(&self, level: u8) -> CoreStyle {
        let theme = palette();
        match level {
            1 | 2 => CoreStyle::new()
                .fg(core_color(theme.header))
                .add_modifier(CoreModifier::BOLD),
            3 => CoreStyle::new()
                .fg(core_color(theme.text))
                .add_modifier(CoreModifier::BOLD),
            _ => CoreStyle::new().fg(core_color(theme.text)),
        }
    }

    fn code(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(palette().text))
    }

    fn link(&self) -> CoreStyle {
        CoreStyle::new()
            .fg(core_color(palette().accent))
            .add_modifier(CoreModifier::UNDERLINED)
    }

    fn blockquote(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(palette().muted))
    }

    fn heading_meta(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(palette().subtle))
    }

    fn metadata_block(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(palette().muted))
    }
}

pub(super) fn core_color(color: Color) -> CoreColor {
    match color {
        Color::Reset => CoreColor::Reset,
        Color::Black => CoreColor::Black,
        Color::Red => CoreColor::Red,
        Color::Green => CoreColor::Green,
        Color::Yellow => CoreColor::Yellow,
        Color::Blue => CoreColor::Blue,
        Color::Magenta => CoreColor::Magenta,
        Color::Cyan => CoreColor::Cyan,
        Color::Gray => CoreColor::Gray,
        Color::DarkGray => CoreColor::DarkGray,
        Color::LightRed => CoreColor::LightRed,
        Color::LightGreen => CoreColor::LightGreen,
        Color::LightYellow => CoreColor::LightYellow,
        Color::LightBlue => CoreColor::LightBlue,
        Color::LightMagenta => CoreColor::LightMagenta,
        Color::LightCyan => CoreColor::LightCyan,
        Color::White => CoreColor::White,
        Color::Rgb(r, g, b) => CoreColor::Rgb(r, g, b),
        Color::Indexed(index) => CoreColor::Indexed(index),
    }
}
