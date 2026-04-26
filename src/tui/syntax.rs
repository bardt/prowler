use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SynStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    fn new() -> Self {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .get("base16-eighties.dark")
            .or_else(|| themes.themes.get("InspiredGitHub"))
            .cloned()
            .unwrap_or_else(|| themes.themes.values().next().cloned().unwrap());
        Self { syntaxes, theme }
    }

    pub fn syntax_for(&self, path: &str) -> &SyntaxReference {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        self.syntaxes
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
    }

    pub fn highlight_line<'a>(
        &self,
        syntax: &SyntaxReference,
        line: &'a str,
    ) -> Vec<(SynStyle, &'a str)> {
        let mut h = HighlightLines::new(syntax, &self.theme);
        h.highlight_line(line, &self.syntaxes)
            .unwrap_or_else(|_| vec![(SynStyle::default(), line)])
    }
}

pub fn highlighter() -> &'static Highlighter {
    static HL: OnceLock<Highlighter> = OnceLock::new();
    HL.get_or_init(Highlighter::new)
}

/// Convert syntect-styled segments to ratatui spans. The `bg_override`
/// parameter is no longer used (kept for callsite compatibility) — diff
/// rows now distinguish themselves via the leading marker and the foreground
/// color, not a row tint, so the user's terminal scheme stays in charge.
///
/// Foreground colors come from a syntect theme baked at compile time; we
/// quantize each RGB to the nearest of the 16 ANSI named colors so the
/// output respects the user's terminal palette.
pub fn to_spans<'a>(
    segments: &[(SynStyle, &'a str)],
    _bg_override: Option<Color>,
) -> Vec<Span<'a>> {
    segments
        .iter()
        .map(|(syn, text)| {
            let mut style = Style::default().fg(rgb_to_ansi(
                syn.foreground.r,
                syn.foreground.g,
                syn.foreground.b,
            ));
            if syn.font_style.contains(FontStyle::BOLD) {
                style = style.add_modifier(Modifier::BOLD);
            }
            if syn.font_style.contains(FontStyle::ITALIC) {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if syn.font_style.contains(FontStyle::UNDERLINE) {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            Span::styled(*text, style)
        })
        .collect()
}

/// Quantize a 24-bit RGB triple to the nearest of the 16 ANSI named colors.
/// We use xterm's default palette as the reference, but the actual on-screen
/// colors come from the user's terminal theme — so terminals with light or
/// custom palettes get a coherent rendering instead of fixed RGB values
/// that ignore them.
fn rgb_to_ansi(r: u8, g: u8, b: u8) -> Color {
    // Default xterm RGB approximations for the 16 ANSI slots.
    const PALETTE: &[(u8, u8, u8, Color)] = &[
        (0, 0, 0, Color::Black),
        (128, 0, 0, Color::Red),
        (0, 128, 0, Color::Green),
        (128, 128, 0, Color::Yellow),
        (0, 0, 128, Color::Blue),
        (128, 0, 128, Color::Magenta),
        (0, 128, 128, Color::Cyan),
        (192, 192, 192, Color::Gray),
        (128, 128, 128, Color::DarkGray),
        (255, 0, 0, Color::LightRed),
        (0, 255, 0, Color::LightGreen),
        (255, 255, 0, Color::LightYellow),
        (0, 0, 255, Color::LightBlue),
        (255, 0, 255, Color::LightMagenta),
        (0, 255, 255, Color::LightCyan),
        (255, 255, 255, Color::White),
    ];
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let mut best = (i32::MAX, Color::Reset);
    for &(pr, pg, pb, c) in PALETTE {
        let (pr, pg, pb) = (pr as i32, pg as i32, pb as i32);
        let dist = (r - pr).pow(2) + (g - pg).pow(2) + (b - pb).pow(2);
        if dist < best.0 {
            best = (dist, c);
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_to_ansi_picks_nearest_named() {
        // Pure red → LightRed (closer to (255, 0, 0) than to (128, 0, 0)).
        assert_eq!(rgb_to_ansi(240, 20, 20), Color::LightRed);
        // Mid grey → DarkGray.
        assert_eq!(rgb_to_ansi(120, 120, 120), Color::DarkGray);
        // Near black → Black.
        assert_eq!(rgb_to_ansi(10, 10, 10), Color::Black);
    }
}
