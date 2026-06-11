//! render snapshots: canonical app states drawn into a `TestBackend` buffer

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::agent::AgentState;
use crate::agent::AgentStatus;
use crate::agent::id::AgentId;
use crate::llm::history::AssistantEvent;
use crate::llm::history::History;
use crate::llm::history::HistoryUpdate;
use crate::llm::history::TurnStatus;
use crate::llm::history::delta::Delta;
use crate::llm::history::delta::DeltaContent;
use crate::llm::history::message::AssistantItem;
use crate::llm::history::message::OutputItem;
use crate::llm::history::message::UserMessage;
use crate::project::Project;
use crate::tui::app::App;
use crate::tui::tab::Tab;

fn render(
    app: &mut App<'_>,
    width: u16,
    height: u16,
) -> String {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    app.draw(&mut term).unwrap();
    term.backend().to_string()
}

fn app_with_tab(
    history: History,
    status: AgentStatus,
) -> App<'static> {
    let mut app = App::new(Project::new_test().unwrap().0, Default::default());
    app.project_name = "demo".into();
    let mut state = AgentState::fake(&app.project);
    state.status = status;
    state.context.history = history;
    state.assistant.config.window = Some(32000);
    let aid = AgentId::from("tab-1".to_string());
    let project = app.project.clone();
    let tab = Tab::new(Some(app.router.clone()), aid.clone(), state, &project);
    app.tabs.insert(aid, tab);
    app.rebuild_tablist();
    app.select_tab(Some(0));
    app
}

fn history(updates: impl IntoIterator<Item = HistoryUpdate>) -> History {
    let mut history = History::new("".into());
    for update in updates {
        history.handle(0, update).unwrap();
    }
    history
}

#[tokio::test]
async fn renders_logo_screen_without_tabs() {
    let mut app = App::new(Project::new_test().unwrap().0, Default::default());
    app.project_name = "demo".into();

    insta::assert_snapshot!(render(&mut app, 80, 24), @r#"
    "┌──────────────────────┐                                                        "
    "│                      │                                                        "
    "│                      │                                                        "
    "│                      │                                                        "
    "│                      │                                                        "
    "│                      │                                                        "
    "│                      │                             ..                         "
    "│                      │8>                         dF                           "
    "│                      │P                    u.   '88bu.                        "
    "│                      │          .    ...ue888b  '*88888bu        .u           "
    "│                      │8u   .udR88N   888R Y888r   ^"*8888N    ud8888.         "
    "│                      │8E` <888'888k  888R I888>  beWE "888L :888'8888.        "
    "│                      │8E  9888 'Y"   888R I888>  888E  888E d888 '88%"        "
    "│                      │8E  9888       888R I888>  888E  888E 8888.+"           "
    "│                      │8E  9888      u8888cJ888   888E  888F 8888L             "
    "│                      │8&  ?8888u../  "*888*P"   .888N..888  '8888c. .+        "
    "│                      │88"  "8888P'     'Y"       `"888*""    "88888%          "
    "│                      │"      "P'                    ""         "YP'           "
    "│                      │                                                        "
    "│                      │                                                        "
    "│                      │                                                        "
    "│                      │                                                        "
    "└──────────────────────┘                                                        "
    "demo                                                                            "
    "#);
}

#[tokio::test]
async fn renders_conversation_tab() {
    let mut app = app_with_tab(
        history([
            HistoryUpdate::UserMessage(UserMessage::new("hello".into(), 0)),
            HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 1 }),
            HistoryUpdate::TurnResponse(AssistantEvent::Started { started_at: 2 }),
            HistoryUpdate::TurnResponse(AssistantEvent::Item(Box::new(AssistantItem::Output(
                OutputItem::new("out-1".into(), 3),
            )))),
            HistoryUpdate::TurnResponse(AssistantEvent::Delta(Delta::new_at(
                "out-1".into(),
                DeltaContent::Output("Hi! How can I help?".into()),
                4,
            ))),
            HistoryUpdate::TurnResponse(AssistantEvent::Completed { ended_at: 5 }),
        ]),
        AgentStatus::Normal(TurnStatus::Idle),
    );
    app.focus = super::AppFocus::Body;

    insta::assert_snapshot!(render(&mut app, 100, 20), @r#"
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "hello                                                                                               "
    "Hi! How can I help?                                                                                 "
    "demo/tab-1                                                               0.6 / 32.0 kT | idle | test"
    "#);
}

#[tokio::test]
async fn renders_failed_tab_with_tablist_overlay() {
    let mut app = app_with_tab(
        history([
            HistoryUpdate::UserMessage(UserMessage::new("hello".into(), 0)),
            HistoryUpdate::TurnResponse(AssistantEvent::Created { created_at: 1 }),
            HistoryUpdate::TurnResponse(AssistantEvent::Failed {
                message: "aborted by user".into(),
                ended_at: 2,
            }),
        ]),
        AgentStatus::Normal(TurnStatus::Failed("aborted by user".into())),
    );

    insta::assert_snapshot!(render(&mut app, 100, 20), @r#"
    "┌──────────────────────┐                                                                            "
    "│ [!]tab-1             │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "│                      │                                                                            "
    "└──────────────────────┘                                                                            "
    "demo/tab-1                                            0.6 / 32.0 kT | failed: aborted by user | test"
    "#);
}
