use anyhow::Context;

#[derive(Clone)]
pub struct AgentDb {
    pub todos: sled::Tree,
    pub todos_by_source: sled::Tree,
    pub notifications: sled::Tree,
    pub gmail_cursor: sled::Tree,
    pub sessions: sled::Tree,
}

impl AgentDb {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let db = sled::open(path).with_context(|| format!("opening sled db at {path}"))?;
        Ok(Self {
            todos: db.open_tree("todos").context("open todos tree")?,
            todos_by_source: db
                .open_tree("todos_by_source")
                .context("open todos_by_source tree")?,
            notifications: db
                .open_tree("notifications")
                .context("open notifications tree")?,
            gmail_cursor: db
                .open_tree("gmail_cursor")
                .context("open gmail_cursor tree")?,
            sessions: db.open_tree("sessions").context("open sessions tree")?,
        })
    }
}
