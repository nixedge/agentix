use agentix_assistant_core::{
    bounded_text, Error, GroceryList, GroceryMutation, GroceryPlan, Intent, PreparedPlan, Record,
    RecordBody, Result, TellStatus,
};

pub(crate) enum Decision {
    Read(PreparedPlan),
    Change(GroceryPlan),
}

pub(crate) fn alias(name: &str) -> bool {
    matches!(
        name.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            .split_whitespace()
            .skip_while(|word| matches!(*word, "my" | "our" | "the" | "a" | "an"))
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
        "groceries"
            | "grocery"
            | "grocery list"
            | "groceries list"
            | "grocery shopping list"
            | "shopping"
            | "shopping list"
            | "ourgroceries"
            | "our groceries"
    )
}

pub(crate) fn local_grocery(record: &Record, records: &[Record]) -> bool {
    match &record.body {
        RecordBody::List { name } => alias(name),
        RecordBody::ListItem { list_id, .. } => records.iter().any(|parent| {
            parent.id == *list_id
                && matches!(&parent.body, RecordBody::List { name } if alias(name))
        }),
        _ => false,
    }
}

fn read(reply: String, status: TellStatus) -> Decision {
    Decision::Read(PreparedPlan {
        commands: vec![],
        status,
        reply,
        records: vec![],
    })
}

pub(crate) fn prepare(
    intent: &Intent,
    originals: &[Record],
    list: Option<&GroceryList>,
    configured: bool,
    request_text: &str,
) -> Result<Option<Decision>> {
    let mut remote = matches!(
        intent,
        Intent::ShowGroceries
            | Intent::AddGroceryItem { .. }
            | Intent::RenameGroceryItem { .. }
            | Intent::SetGroceryItemCompleted { .. }
            | Intent::DeleteGroceryItem { .. }
    );
    if configured {
        remote |= matches!(intent, Intent::AddListItem {list_name, ..} | Intent::ShowList {list_name, ..} if alias(list_name) || list.is_some_and(|list| list.name.eq_ignore_ascii_case(list_name.trim())));
        if let Intent::EditListItem { id, .. } | Intent::DeleteRecord { id, .. } = intent {
            if originals
                .iter()
                .any(|record| record.id == *id && local_grocery(record, originals))
            {
                return Ok(Some(read("Please choose the item from the current OurGroceries list; the old local grocery list is not used for remote edits.".into(), TellStatus::Clarification)));
            }
        }
    }
    if !remote {
        return Ok(None);
    }
    let Some(list) = list.filter(|_| configured) else {
        return Ok(Some(read("OurGroceries is not configured or is currently unavailable. I did not change a local or remote grocery list.".into(), TellStatus::Failed)));
    };
    list.validate()?;
    let target = match intent {
        Intent::RenameGroceryItem { item_id, .. }
        | Intent::SetGroceryItemCompleted { item_id, .. }
        | Intent::DeleteGroceryItem { item_id } => Some(item_id),
        _ => None,
    };
    if let Some(id) = target {
        if let Some(original) = list.items.iter().find(|item| item.id == *id) {
            let duplicates = list
                .items
                .iter()
                .filter(|item| {
                    item.text.eq_ignore_ascii_case(&original.text)
                        && item.completed == original.completed
                })
                .collect::<Vec<_>>();
            let explicit = format!("id:{id}");
            if duplicates.len() > 1 && !request_text.split_whitespace().any(|part| part == explicit)
            {
                return Ok(Some(read(format!("OurGroceries has multiple matching items. Specify the exact item as id: followed by one of these IDs: {}.", duplicates.iter().take(8).map(|item| item.id.as_str()).collect::<Vec<_>>().join(", ")), TellStatus::Clarification)));
            }
        }
    }
    let mutation = match intent {
        Intent::ShowGroceries | Intent::ShowList { .. } => {
            let labels = list
                .items
                .iter()
                .filter(|item| !item.completed)
                .take(20)
                .map(|item| item.text.chars().take(60).collect::<String>())
                .collect::<Vec<_>>();
            let count = list.items.iter().filter(|item| !item.completed).count();
            let name = list.name.chars().take(100).collect::<String>();
            let reply = if labels.is_empty() {
                format!("OurGroceries: {name} has no items left to buy.")
            } else {
                format!(
                    "OurGroceries, {name}: {}.{}",
                    labels.join(", "),
                    if count > 20 {
                        " Showing the first twenty needed items."
                    } else {
                        ""
                    }
                )
            };
            return Ok(Some(read(reply, TellStatus::Answer)));
        }
        Intent::AddGroceryItem { text } | Intent::AddListItem { text, .. } => {
            bounded_text(text, 1_024)?;
            GroceryMutation::Add { text: text.clone() }
        }
        Intent::RenameGroceryItem { item_id, text } => {
            bounded_text(text, 1_024)?;
            let original = list
                .items
                .iter()
                .find(|item| item.id == *item_id)
                .ok_or_else(|| {
                    Error::Invalid("grocery target is not in the current snapshot".into())
                })?
                .clone();
            if original.text == *text {
                return Ok(Some(read(
                    "OurGroceries: the item already has that name.".into(),
                    TellStatus::Answer,
                )));
            }
            GroceryMutation::Rename {
                original,
                text: text.clone(),
            }
        }
        Intent::SetGroceryItemCompleted { item_id, completed } => {
            let original = list
                .items
                .iter()
                .find(|item| item.id == *item_id)
                .ok_or_else(|| {
                    Error::Invalid("grocery target is not in the current snapshot".into())
                })?
                .clone();
            if original.completed == *completed {
                return Ok(Some(read(
                    "OurGroceries: the item already has that purchase state.".into(),
                    TellStatus::Answer,
                )));
            }
            GroceryMutation::SetCompleted {
                original,
                completed: *completed,
            }
        }
        Intent::DeleteGroceryItem { item_id } => {
            let original = list
                .items
                .iter()
                .find(|item| item.id == *item_id)
                .ok_or_else(|| {
                    Error::Invalid("grocery target is not in the current snapshot".into())
                })?
                .clone();
            GroceryMutation::Delete { original }
        }
        _ => return Err(Error::Invalid("unsupported grocery instruction".into())),
    };
    let plan = GroceryPlan {
        team_id: list.team_id.clone(),
        list_id: list.id.clone(),
        list_name: list.name.clone(),
        mutation,
    };
    plan.validate()?;
    Ok(Some(Decision::Change(plan)))
}

pub(crate) fn context(list: &GroceryList) -> GroceryList {
    let mut items = list.items.clone();
    items.sort_by_key(|item| item.completed);
    let mut bytes = 0;
    items.retain_mut(|item| {
        item.note = None;
        item.category_id = None;
        let size = item.id.len() + item.text.len() + 128;
        bytes += size;
        bytes <= 6_144
    });
    items.truncate(32);
    GroceryList {
        team_id: list.team_id.clone(),
        id: list.id.clone(),
        name: list.name.clone(),
        items,
    }
}
