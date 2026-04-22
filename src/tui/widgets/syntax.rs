use ansi_to_tui::IntoText;
use anyhow::Context;
use anyhow::Result;
use ratatui::text::Text;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxReference;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use syntect::util::as_24_bit_terminal_escaped;

pub static HIGHLIGHTER: std::sync::LazyLock<Highlighter> = std::sync::LazyLock::new(|| {
    Highlighter::new().expect("failed to initialize syntax highlighter")
});

pub struct Highlighter {
    set: SyntaxSet,

    theme: Theme,

    pub diff: SyntaxReference,
    pub bash: SyntaxReference,
    pub markdown: SyntaxReference,
}

impl Highlighter {
    fn new() -> Result<Self> {
        let set = SyntaxSet::load_defaults_newlines();
        let themeset = ThemeSet::load_defaults();
        let theme = themeset.themes["base16-ocean.dark"].clone();
        Ok(Self {
            diff: set
                .find_syntax_by_token("diff")
                .context("diff syntax not found")?
                .clone(),
            bash: set
                .find_syntax_by_token("bash")
                .context("bash syntax not found")?
                .clone(),
            markdown: set
                .find_syntax_by_token("markdown")
                .context("markdown syntax not found")?
                .clone(),

            set,
            theme,
        })
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
