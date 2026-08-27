pub mod calendar;
pub mod gmail;
pub mod ntfy;
pub mod todo;

use std::sync::Arc;

use agentix_harness::AgentLoop;

use crate::scheduler::AppState;

fn system_prompt() -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d");
    format!(
        "You are a home assistant agent for Samuel Leathers. Today's date is {today} (UTC). \
You have tools to manage his todo list, Google Calendar, and Gmail. Always use the available \
tools to answer questions — never say you cannot access the todo list or calendar, because you \
have direct tool access. When asked what todos exist, call todo_list. When creating tasks, call \
todo_create. When given relative dates like \"tomorrow\" or \"next Friday\", compute the exact \
ISO 8601 date from today's date. Be concise and action-oriented. \
Respond in plain conversational sentences — no markdown, no code blocks, no bullet points, \
no backticks, no headers. Speak as if answering out loud."
    )
}

pub fn build_agent_loop(gateway_url: &str, model: &str, state: &Arc<AppState>) -> AgentLoop {
    let mut agent = AgentLoop::new(gateway_url, model)
        .with_system_prompt(system_prompt())
        .with_tool(Box::new(todo::TodoListTool(state.todo_store())))
        .with_tool(Box::new(todo::TodoCreateTool(state.todo_store())))
        .with_tool(Box::new(todo::TodoCompleteTool(state.todo_store())))
        .with_tool(Box::new(todo::TodoUpdateTool(state.todo_store())));

    if let Some(auth) = &state.google_auth {
        if let Some(ref google_cfg) = state.config.google {
            let cal = Arc::new(crate::google::calendar::CalendarClient::new(
                Arc::clone(auth),
                google_cfg.calendar_id.clone(),
            ));
            agent = agent
                .with_tool(Box::new(calendar::CalendarListEventsTool(Arc::clone(&cal))))
                .with_tool(Box::new(calendar::CalendarCreateEventTool(cal)));

            let gmail_client = Arc::new(crate::google::gmail::GmailClient::new(Arc::clone(auth)));
            agent = agent.with_tool(Box::new(gmail::GmailScanInboxTool(gmail_client)));
        }
    }

    agent
}
