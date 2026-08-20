pub mod calendar;
pub mod gmail;
pub mod ntfy;
pub mod todo;

use std::sync::Arc;

use agentix_harness::AgentLoop;

use crate::scheduler::AppState;

pub fn build_agent_loop(gateway_url: &str, model: &str, state: &Arc<AppState>) -> AgentLoop {
    let mut agent = AgentLoop::new(gateway_url, model)
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
