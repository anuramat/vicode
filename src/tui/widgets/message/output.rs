use crate::llm::message::OutputContent;
use crate::llm::message::OutputItem;
use crate::tui::widgets::container::element::*;
use crate::tui::widgets::markdown::*;

impl From<&OutputItem> for Element {
    fn from(msg: &OutputItem) -> Self {
        let text: String = msg
            .content
            .iter()
            .map(|c| match c {
                OutputContent::Text(t) => t.clone(),
                OutputContent::Refusal(refusal_content) => {
                    unimplemented!("refusal: {:?}", refusal_content)
                }
            })
            .collect();
        let widget: MarkdownWidget = text.into();
        widget.into()
    }
}
