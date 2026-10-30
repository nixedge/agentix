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

fn failure(stage: &'static str, reason: &'static str) -> Error {
    tracing::warn!(stage, reason, "OurGroceries request failed");
    Error::Groceries
}

fn transport_failure(stage: &'static str, error: reqwest::Error) -> Error {
    let reason = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection_or_tls"
    } else if error.is_body() {
        "response_body"
    } else {
        "http_transport"
    };
    failure(stage, reason)
}

fn status_failure(stage: &'static str, status: reqwest::StatusCode) -> Error {
    tracing::warn!(stage, status = status.as_u16(), "OurGroceries HTTP failure");
    Error::Groceries
}

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

    async fn bytes(mut response: reqwest::Response, stage: &'static str) -> Result<Vec<u8>> {
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE as u64)
        {
            return Err(failure(stage, "response_too_large"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| transport_failure(stage, error))?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err(failure(stage, "response_too_large"));
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
            .map_err(|error| transport_failure("sign_in", error))?;
        if !(response.status().is_success() || response.status().is_redirection()) {
            return Err(status_failure("sign_in", response.status()));
        }
        let mut session_cookie = None;
        for cookie in response.headers().get_all(header::SET_COOKIE) {
            let pair = cookie
                .to_str()
                .map_err(|_| failure("sign_in", "invalid_cookie_header"))?
                .split(';')
                .next()
                .ok_or_else(|| failure("sign_in", "invalid_cookie_header"))?;
            if let Some(value) = pair.strip_prefix("ourgroceries-auth=") {
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_graphic() && !matches!(b, b',' | b';' | b'"' | b'\\'))
                {
                    return Err(failure("sign_in", "invalid_session_cookie"));
                }
                session_cookie = Some(
                    header::HeaderValue::from_str(pair)
                        .map_err(|_| failure("sign_in", "invalid_session_cookie"))?,
                );
            }
        }
        let cookie = session_cookie.ok_or_else(|| failure("sign_in", "missing_session_cookie"))?;
        // Never follow a login redirect: even a same-origin path could be unexpected.
        let response = self
            .http
            .get(format!("{}/your-lists/", self.origin))
            .header(header::COOKIE, cookie.clone())
            .send()
            .await
            .map_err(|error| transport_failure("household_page", error))?;
        if !response.status().is_success() {
            return Err(status_failure("household_page", response.status()));
        }
        let html = String::from_utf8(Self::bytes(response, "household_page").await?)
            .map_err(|_| failure("household_page", "invalid_utf8"))?;
        let team =
            team_id(&html).map_err(|_| failure("household_page", "invalid_or_missing_team_id"))?;
        Ok(Session { cookie, team })
    }

    async fn rpc(&self, command: &'static str, mut payload: Value) -> Result<Value> {
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
            .map_err(|error| transport_failure(command, error))?;
        if !response.status().is_success() {
            *session = None;
            return Err(status_failure(command, response.status()));
        }
        let result = Self::bytes(response, command).await.and_then(|bytes| {
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| failure(command, "invalid_json"))?;
            if !value.is_object()
                || value
                    .get("error")
                    .is_some_and(|error| !error.is_null() && error != &Value::Bool(false))
            {
                return Err(failure(command, "remote_error_or_invalid_envelope"));
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
                .ok_or_else(|| failure("getOverview", "missing_shopping_lists"))?,
        )
        .map_err(|_| failure("getOverview", "invalid_shopping_lists"))?;
        if lists.len() > 2_000 {
            return Err(Error::Groceries);
        }
        for list in &lists {
            grocery_id(&list.id)?;
            bounded_text(&list.name, 256)?;
        }
        Ok(lists)
    }

    pub async fn inspect(&self) -> Result<Value> {
        let id = self
            .list_id
            .as_ref()
            .ok_or_else(|| Error::Invalid("no OurGroceries list selected".into()))?;
        self.rpc("getList", json!({"listId":id}))
            .await?
            .get("list")
            .cloned()
            .ok_or_else(|| failure("getList", "missing_list"))
    }

    async fn list(&self) -> Result<GroceryList> {
        let id = self
            .list_id
            .as_ref()
            .ok_or_else(|| Error::Invalid("no OurGroceries list selected".into()))?;
        let raw = self.inspect().await?;
        let remote: RemoteList =
            serde_json::from_value(raw.clone()).map_err(|_| list_decode_failure(&raw))?;
        if remote.id.as_ref().is_some_and(|remote_id| remote_id != id) {
            return Err(failure("getList", "list_id_mismatch"));
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
        list.validate()
            .map_err(|_| failure("getList", "list_validation"))?;
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

fn json_kind(value: Option<&Value>) -> &'static str {
    match value {
        None => "missing",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn list_decode_issue(raw: &Value) -> (&'static str, &'static str, &'static str) {
    if !raw.is_object() {
        return ("list", "object", json_kind(Some(raw)));
    }
    for (key, field) in [("id", "list.id"), ("name", "list.name")] {
        if let Some(value) = raw
            .get(key)
            .filter(|value| !value.is_null() && !value.is_string())
        {
            return (field, "string_or_null", json_kind(Some(value)));
        }
    }
    let Some(items) = raw.get("items").and_then(Value::as_array) else {
        return ("list.items", "array", json_kind(raw.get("items")));
    };
    for item in items {
        if !item.is_object() {
            return ("list.items[]", "object", json_kind(Some(item)));
        }
        if item.get("name").is_some() && item.get("value").is_some() {
            return (
                "list.items[].text",
                "one_name_or_value_field",
                "both_fields",
            );
        }
        for (value, field, expected) in [
            (item.get("id"), "list.items[].id", "string"),
            (
                item.get("name").or_else(|| item.get("value")),
                "list.items[].text",
                "string",
            ),
            (
                item.get("categoryId"),
                "list.items[].categoryId",
                "string_or_null",
            ),
            (item.get("note"), "list.items[].note", "string_or_null"),
            (
                item.get("crossedOff"),
                "list.items[].crossedOff",
                "boolean_if_present",
            ),
        ] {
            let valid = match expected {
                "string" => value.is_some_and(Value::is_string),
                "string_or_null" => value.is_none_or(|value| value.is_null() || value.is_string()),
                _ => value.is_none_or(Value::is_boolean),
            };
            if !valid {
                return (field, expected, json_kind(value));
            }
        }
    }
    ("list", "supported_list_schema", "undetermined_mismatch")
}

fn list_decode_failure(raw: &Value) -> Error {
    let (field, expected, actual) = list_decode_issue(raw);
    tracing::warn!(
        stage = "getList",
        reason = "invalid_list_fields",
        field,
        expected,
        actual,
        "OurGroceries list decoding failed"
    );
    Error::Groceries
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
