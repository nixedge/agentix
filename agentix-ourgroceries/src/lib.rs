//! OurGroceries protocol adapted from ljmerza/py-our-groceries (MIT); see NOTICE.md.
use agentix_assistant_core::{
    bounded_text, grocery_id, Error, Groceries, GroceryItem, GroceryList, GroceryMutation,
    GroceryOutcome, GroceryPlan, Result,
};
use async_trait::async_trait;
use reqwest::{header, Client as HttpClient};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::Mutex;

const ORIGIN: &str = "https://www.ourgroceries.com";
const MAX_RESPONSE: usize = 2_097_152;

struct Session {
    cookie: header::HeaderValue,
    team: String,
}

// Credentials and cookies deliberately do not implement Debug or Serialize.
pub struct Client {
    http: HttpClient,
    origin: String,
    username: String,
    password: String,
    list_id: Option<String>,
    session: Mutex<Option<Session>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSummary {
    pub id: String,
    pub name: String,
}

impl Client {
    #[cfg(feature = "test-support")]
    pub fn for_local_test(origin: &str, list_id: &str) -> Result<Self> {
        let url = reqwest::Url::parse(origin).map_err(|_| Error::Groceries)?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Groceries);
        }
        Self::at_origin(
            origin,
            "fixture@example.org".into(),
            "fixture-password".into(),
            Some(list_id.into()),
        )
    }

    pub fn new(username: String, password: String, list_id: Option<String>) -> Result<Self> {
        Self::at_origin(ORIGIN, username, password, list_id)
    }

    fn at_origin(
        origin: &str,
        username: String,
        password: String,
        list_id: Option<String>,
    ) -> Result<Self> {
        bounded_text(&username, 320)?;
        bounded_text(&password, 4_096)?;
        if let Some(id) = &list_id {
            grocery_id(id)?;
        }
        let http = HttpClient::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Error::Groceries)?;
        Ok(Self {
            http,
            origin: origin.trim_end_matches('/').into(),
            username,
            password,
            list_id,
            session: Mutex::new(None),
        })
    }

    async fn bytes(mut response: reqwest::Response) -> Result<Vec<u8>> {
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE as u64)
        {
            return Err(Error::Groceries);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Groceries)? {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err(Error::Groceries);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn login(&self) -> Result<Session> {
        let response = self
            .http
            .post(format!("{}/sign-in", self.origin))
            .form(&[
                ("emailAddress", self.username.as_str()),
                ("password", self.password.as_str()),
                ("action", "sign-in"),
            ])
            .send()
            .await
            .map_err(|_| Error::Groceries)?;
        if !(response.status().is_success() || response.status().is_redirection()) {
            return Err(Error::Groceries);
        }
        let mut session_cookie = None;
        for cookie in response.headers().get_all(header::SET_COOKIE) {
            let pair = cookie
                .to_str()
                .map_err(|_| Error::Groceries)?
                .split(';')
                .next()
                .ok_or(Error::Groceries)?;
            if let Some(value) = pair.strip_prefix("ourgroceries-auth=") {
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && !matches!(b, b',' | b';' | b'"' | b'\\'))
                {
                    return Err(Error::Groceries);
                }
                session_cookie =
                    Some(header::HeaderValue::from_str(pair).map_err(|_| Error::Groceries)?);
            }
        }
        let cookie = session_cookie.ok_or(Error::Groceries)?;
        // Never follow a login redirect: even a same-origin path could be unexpected.
        let response = self
            .http
            .get(format!("{}/your-lists/", self.origin))
            .header(header::COOKIE, cookie.clone())
            .send()
            .await
            .map_err(|_| Error::Groceries)?;
        if !response.status().is_success() {
            return Err(Error::Groceries);
        }
        let html = String::from_utf8(Self::bytes(response).await?).map_err(|_| Error::Groceries)?;
        let team = team_id(&html)?;
        Ok(Session { cookie, team })
    }

    async fn rpc(&self, command: &str, mut payload: Value) -> Result<Value> {
        let mut session = self.session.lock().await;
        if session.is_none() {
            *session = Some(self.login().await?);
        }
        let current = session.as_ref().ok_or(Error::Groceries)?;
        payload["command"] = json!(command);
        payload["teamId"] = json!(current.team);
        let response = self
            .http
            .post(format!("{}/your-lists/", self.origin))
            .header(header::COOKIE, current.cookie.clone())
            .json(&payload)
            .send()
            .await
            .map_err(|_| Error::Groceries)?;
        if !response.status().is_success() {
            *session = None;
            return Err(Error::Groceries);
        }
        let result = Self::bytes(response).await.and_then(|bytes| {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Groceries)?;
            if !value.is_object()
                || value
                    .get("error")
                    .is_some_and(|error| !error.is_null() && error != &Value::Bool(false))
            {
                return Err(Error::Groceries);
            }
            Ok(value)
        });
        if result.is_err() {
            *session = None;
        }
        result
    }

    pub async fn lists(&self) -> Result<Vec<ListSummary>> {
        let response = self.rpc("getOverview", json!({})).await?;
        let lists: Vec<ListSummary> = serde_json::from_value(
            response
                .get("shoppingLists")
                .cloned()
                .ok_or(Error::Groceries)?,
        )
        .map_err(|_| Error::Groceries)?;
        if lists.len() > 2_000 {
            return Err(Error::Groceries);
        }
        for list in &lists {
            grocery_id(&list.id)?;
            bounded_text(&list.name, 256)?;
        }
        Ok(lists)
    }

    async fn list(&self) -> Result<GroceryList> {
        let id = self
            .list_id
            .as_ref()
            .ok_or_else(|| Error::Invalid("no OurGroceries list selected".into()))?;
        let response = self.rpc("getList", json!({"listId":id})).await?;
        let remote: RemoteList =
            serde_json::from_value(response.get("list").cloned().ok_or(Error::Groceries)?)
                .map_err(|_| Error::Groceries)?;
        if remote.id.as_ref().is_some_and(|remote_id| remote_id != id) {
            return Err(Error::Groceries);
        }
        let list = GroceryList {
            team_id: self
                .session
                .lock()
                .await
                .as_ref()
                .ok_or(Error::Groceries)?
                .team
                .clone(),
            id: id.clone(),
            name: remote.name.unwrap_or_else(|| "Groceries".into()),
            items: remote
                .items
                .into_iter()
                .map(|item| GroceryItem {
                    id: item.id,
                    text: item.name,
                    category_id: item.category_id,
                    note: item.note,
                    completed: item.crossed_off
                        || item.crossed_off_at.as_ref().is_some_and(|at| {
                            !at.is_null() && at != &json!(0) && at != &json!(false)
                        }),
                })
                .collect(),
        };
        list.validate().map_err(|_| Error::Groceries)?;
        Ok(list)
    }

    async fn mutate(&self, plan: &GroceryPlan) -> GroceryOutcome {
        if plan.validate().is_err() || self.list_id.as_ref() != Some(&plan.list_id) {
            return GroceryOutcome::Rejected;
        }
        let Ok(before) = self.list().await else {
            return GroceryOutcome::Rejected;
        };
        if before.team_id != plan.team_id {
            return GroceryOutcome::Rejected;
        }
        let original = match &plan.mutation {
            GroceryMutation::Add { .. } => None,
            GroceryMutation::Rename { original, .. }
            | GroceryMutation::SetCompleted { original, .. }
            | GroceryMutation::Delete { original } => Some(original),
        };
        if let Some(original) = original {
            if before.items.iter().find(|item| item.id == original.id) != Some(original) {
                return GroceryOutcome::Rejected;
            }
        }
        let (command, mut payload) = match &plan.mutation {
            GroceryMutation::Add { text } => (
                "insertItem",
                json!({"value":text,"categoryId":"uncategorized","note":null}),
            ),
            GroceryMutation::Rename { original, text } => (
                "changeItemValue",
                json!({"itemId":original.id,"newValue":text,"categoryId":original.category_id}),
            ),
            GroceryMutation::SetCompleted {
                original,
                completed,
            } => (
                "setItemCrossedOff",
                json!({"itemId":original.id,"crossedOff":completed}),
            ),
            GroceryMutation::Delete { original } => ("deleteItem", json!({"itemId":original.id})),
        };
        payload["listId"] = json!(plan.list_id);
        // There is deliberately no retry, including after auth failures or timeouts.
        if self.rpc(command, payload).await.is_err() {
            return GroceryOutcome::Uncertain;
        }
        let Ok(after) = self.list().await else {
            return GroceryOutcome::Uncertain;
        };
        let confirmed = match &plan.mutation {
            GroceryMutation::Add { text } => after
                .items
                .iter()
                .any(|item| item.text == *text && !item.completed),
            GroceryMutation::Rename { original, text } => {
                let mut expected = original.clone();
                expected.text.clone_from(text);
                after.items.iter().any(|item| item == &expected)
            }
            GroceryMutation::SetCompleted {
                original,
                completed,
            } => {
                let mut expected = original.clone();
                expected.completed = *completed;
                after.items.iter().any(|item| item == &expected)
            }
            GroceryMutation::Delete { original } => {
                !after.items.iter().any(|item| item.id == original.id)
            }
        };
        if confirmed {
            GroceryOutcome::Confirmed
        } else {
            GroceryOutcome::Uncertain
        }
    }
}

fn team_id(html: &str) -> Result<String> {
    let mut remaining = html
        .split_once("g_teamId")
        .ok_or(Error::Groceries)?
        .1
        .trim_start();
    remaining = remaining
        .strip_prefix('=')
        .ok_or(Error::Groceries)?
        .trim_start();
    let mut stream = serde_json::Deserializer::from_str(remaining).into_iter::<String>();
    let id = stream
        .next()
        .ok_or(Error::Groceries)?
        .map_err(|_| Error::Groceries)?;
    grocery_id(&id).map_err(|_| Error::Groceries)?;
    Ok(id)
}

#[derive(Deserialize)]
struct RemoteList {
    id: Option<String>,
    name: Option<String>,
    items: Vec<RemoteItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteItem {
    id: String,
    #[serde(alias = "value")]
    name: String,
    category_id: Option<String>,
    note: Option<String>,
    #[serde(default)]
    crossed_off: bool,
    crossed_off_at: Option<Value>,
}

#[async_trait]
impl Groceries for Client {
    async fn snapshot(&self) -> Result<GroceryList> {
        self.list().await
    }
    async fn execute(&self, plan: &GroceryPlan) -> GroceryOutcome {
        self.mutate(plan).await
    }
}

#[cfg(test)]
mod tests;
