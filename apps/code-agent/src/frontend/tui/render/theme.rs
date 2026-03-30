use ratatui::style::Color;
use ratatui_core::style::{Color as CoreColor, Modifier as CoreModifier, Style as CoreStyle};
use tui_markdown::StyleSheet as MarkdownStyleSheet;

pub(super) const BG: Color = Color::Rgb(15, 17, 20);
pub(super) const MAIN_BG: Color = Color::Rgb(17, 19, 22);
pub(super) const FOOTER_BG: Color = Color::Rgb(20, 22, 26);
pub(super) const BOTTOM_PANE_BG: Color = Color::Rgb(24, 27, 31);
pub(super) const BORDER_ACTIVE: Color = Color::Rgb(165, 168, 160);
pub(super) const TEXT: Color = Color::Rgb(235, 236, 232);
pub(super) const MUTED: Color = Color::Rgb(154, 158, 151);
pub(super) const SUBTLE: Color = Color::Rgb(98, 103, 108);
pub(super) const ACCENT: Color = Color::Rgb(108, 189, 182);
pub(super) const USER: Color = Color::Rgb(221, 188, 128);
pub(super) const ASSISTANT: Color = Color::Rgb(150, 201, 171);
pub(super) const ERROR: Color = Color::Rgb(227, 125, 118);
pub(super) const WARN: Color = Color::Rgb(223, 179, 88);
pub(super) const HEADER: Color = Color::Rgb(244, 244, 239);

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct NanoclawMarkdownStyleSheet;

impl MarkdownStyleSheet for NanoclawMarkdownStyleSheet {
    fn heading(&self, level: u8) -> CoreStyle {
        match level {
            1 | 2 => CoreStyle::new()
                .fg(core_color(HEADER))
                .add_modifier(CoreModifier::BOLD),
            3 => CoreStyle::new()
                .fg(core_color(TEXT))
                .add_modifier(CoreModifier::BOLD),
            _ => CoreStyle::new().fg(core_color(TEXT)),
        }
    }

    fn code(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(TEXT))
    }

    fn link(&self) -> CoreStyle {
        CoreStyle::new()
            .fg(core_color(ACCENT))
            .add_modifier(CoreModifier::UNDERLINED)
    }

    fn blockquote(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(MUTED))
    }

    fn heading_meta(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(SUBTLE))
    }

    fn metadata_block(&self) -> CoreStyle {
        CoreStyle::new().fg(core_color(MUTED))
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
