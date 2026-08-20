pub mod ntfy;
pub mod todo;

use std::sync::Arc;

use agentix_harness::AgentLoop;

use crate::scheduler::AppState;

pub fn build_agent_loop(gateway_url: &str, model: &str, state: &Arc<AppState>) -> AgentLoop {
    AgentLoop::new(gateway_url, model)
        .with_tool(Box::new(todo::TodoListTool(state.todo_store())))
        .with_tool(Box::new(todo::TodoCreateTool(state.todo_store())))
        .with_tool(Box::new(todo::TodoCompleteTool(state.todo_store())))
        .with_tool(Box::new(todo::TodoUpdateTool(state.todo_store())))
}
