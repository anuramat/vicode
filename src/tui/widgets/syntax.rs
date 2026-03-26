use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::text::Text;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxReference;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use syntect::util::as_24_bit_terminal_escaped;

pub static HIGHLIGHTER: std::sync::LazyLock<Highlighter> =
    std::sync::LazyLock::new(Highlighter::new);

pub struct Highlighter {
    set: SyntaxSet,
    themeset: ThemeSet,

    theme: Theme,

    pub diff: SyntaxReference,
    pub bash: SyntaxReference,
    pub markdown: SyntaxReference,
}

impl Highlighter {
    fn new() -> Self {
        let set = SyntaxSet::load_defaults_newlines();
        let diff = set.find_syntax_by_token("diff").unwrap().clone();
        let bash = set.find_syntax_by_token("bash").unwrap().clone();
        let markdown = set.find_syntax_by_token("markdown").unwrap().clone();
        let themeset = ThemeSet::load_defaults();
        let theme = themeset.themes["base16-ocean.dark"].clone(); // TODO parameterize theme
        // XXX
        Self {
            set,
            themeset,

            theme,

            diff,
            bash,
            markdown,
        }
    }

    pub fn highlight(
        &self,
        code: &str,
        syntax: &SyntaxReference,
    ) -> Text<'static> {
        self.try_highlight(code, syntax).unwrap_or_else(|e| {
            tracing::warn!("syntax highlighting failed: {e}");
            Text::from(code.to_string())
        })
    }

    fn try_highlight(
        &self,
        code: &str,
        syntax: &SyntaxReference,
    ) -> Result<Text<'static>> {
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let mut text = Text::default();
        for line in LinesWithEndings::from(code) {
            text.extend(
                as_24_bit_terminal_escaped(&highlighter.highlight_line(line, &self.set)?, false)
                    .into_text()?,
            );
        }
        Ok(text)
    }
}
