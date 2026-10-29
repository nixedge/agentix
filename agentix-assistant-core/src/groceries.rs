use crate::{bounded_text, Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub fn grocery_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 256 || !id.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(Error::Invalid("invalid remote grocery identity".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryItem {
    pub id: String,
    pub text: String,
    pub completed: bool,
    pub category_id: Option<String>,
    pub note: Option<String>,
}

impl GroceryItem {
    pub fn validate(&self) -> Result<()> {
        grocery_id(&self.id)?;
        bounded_text(&self.text, 1_024)?;
        if let Some(category) = &self.category_id {
            grocery_id(category)?;
        }
        if self.note.as_ref().is_some_and(|note| note.len() > 8_192) {
            return Err(Error::Invalid("remote grocery note exceeds limit".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryList {
    pub team_id: String,
    pub id: String,
    pub name: String,
    pub items: Vec<GroceryItem>,
}

impl GroceryList {
    pub fn validate(&self) -> Result<()> {
        grocery_id(&self.team_id)?;
        grocery_id(&self.id)?;
        bounded_text(&self.name, 256)?;
        if self.items.len() > 2_000 {
            return Err(Error::Invalid("remote grocery list exceeds limit".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for item in &self.items {
            item.validate()?;
            if !seen.insert(&item.id) {
                return Err(Error::Invalid("duplicate remote grocery identity".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum GroceryMutation {
    Add {
        text: String,
    },
    Rename {
        original: GroceryItem,
        text: String,
    },
    SetCompleted {
        original: GroceryItem,
        completed: bool,
    },
    Delete {
        original: GroceryItem,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroceryPlan {
    pub team_id: String,
    pub list_id: String,
    pub list_name: String,
    pub mutation: GroceryMutation,
}

impl GroceryPlan {
    pub fn response(
        &self,
        request: &crate::TellRequest,
        outcome: GroceryOutcome,
    ) -> crate::TellResponse {
        crate::TellResponse {
            request_id: request.request_id,
            conversation_id: request.conversation_id,
            status: crate::TellStatus::Answer,
            reply: self.reply(outcome),
            outcomes: vec![],
            records: vec![],
        }
    }

    pub fn validate(&self) -> Result<()> {
        grocery_id(&self.team_id)?;
        grocery_id(&self.list_id)?;
        bounded_text(&self.list_name, 256)?;
        match &self.mutation {
            GroceryMutation::Add { text } => bounded_text(text, 1_024),
            GroceryMutation::Rename { original, text } => {
                original.validate()?;
                bounded_text(text, 1_024)
            }
            GroceryMutation::SetCompleted { original, .. }
            | GroceryMutation::Delete { original } => original.validate(),
        }
    }

    pub fn reply(&self, outcome: GroceryOutcome) -> String {
        match outcome {
            GroceryOutcome::Uncertain => "OurGroceries did not confirm the change. Check your list before repeating it; I will not automatically send this change again.".into(),
            GroceryOutcome::Rejected => "OurGroceries was unavailable or the item changed before I could apply the instruction. I did not send the change. Read the list and try again.".into(),
            GroceryOutcome::Confirmed => {
                let name: String = self.list_name.chars().take(100).collect();
                match &self.mutation {
                    GroceryMutation::Add { text } => format!("OurGroceries: added {} to {}.", text.chars().take(200).collect::<String>(), name),
                    GroceryMutation::Rename { .. } => format!("OurGroceries: updated the item in {name}."),
                    GroceryMutation::SetCompleted { completed, .. } => format!("OurGroceries: marked the item {} in {name}.", if *completed { "purchased" } else { "needed again" }),
                    GroceryMutation::Delete { .. } => format!("OurGroceries: removed the item from {name}."),
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroceryOutcome {
    Confirmed,
    Rejected,
    Uncertain,
}

#[async_trait]
pub trait Groceries: Send + Sync {
    async fn snapshot(&self) -> Result<GroceryList>;
    // The caller must durably claim dispatch before invoking this exactly once.
    async fn execute(&self, plan: &GroceryPlan) -> GroceryOutcome;
}
