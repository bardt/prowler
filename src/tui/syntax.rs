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

/// Convert syntect-styled segments to ratatui spans, applying an optional
/// override background (used to colour the row by diff status).
pub fn to_spans<'a>(
    segments: &[(SynStyle, &'a str)],
    bg_override: Option<Color>,
) -> Vec<Span<'a>> {
    segments
        .iter()
        .map(|(syn, text)| {
            let mut style = Style::default().fg(syn_color(syn.foreground));
            if let Some(bg) = bg_override {
                style = style.bg(bg);
            }
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

fn syn_color(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}
