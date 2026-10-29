use agentix_assistant_core::{
    bounded_text, Command, CommandRequest, Error, Intent, PreparedPlan, Record, RecordBody, Result,
    TellStatus,
};
use uuid::Uuid;

fn command(command: Command) -> CommandRequest {
    CommandRequest {
        request_id: Uuid::new_v4(),
        command,
    }
}
fn create(body: RecordBody) -> CommandRequest {
    command(Command::Create {
        id: Uuid::new_v4(),
        body,
    })
}
fn preview(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
fn normalize(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn matching_lists<'a>(
    records: &'a [Record],
    name: &str,
    id: Option<Uuid>,
) -> Result<Vec<&'a Record>> {
    bounded_text(name, 256)?;
    Ok(records
        .iter()
        .filter(|record| {
            !record.deleted
                && match &record.body {
                    RecordBody::List { name: existing } => id.map_or_else(
                        || normalize(name) == normalize(existing),
                        |id| record.id == id,
                    ),
                    _ => false,
                }
        })
        .collect())
}

fn clarify(question: String) -> PreparedPlan {
    PreparedPlan {
        commands: vec![],
        status: TellStatus::Clarification,
        reply: format!("Please clarify: {question}"),
        records: vec![],
    }
}

pub(crate) fn prepare(intent: Intent, records: &[Record]) -> Result<PreparedPlan> {
    let mut plan = PreparedPlan {
        commands: vec![],
        status: TellStatus::Saved,
        reply: String::new(),
        records: vec![],
    };
    match intent {
        Intent::AddListItem {
            list_name,
            list_id,
            text,
        } => {
            bounded_text(&text, 1_024)?;
            let matches = matching_lists(records, &list_name, list_id)?;
            let (parent, name) = match matches.as_slice() {
                [record] => (
                    record.id,
                    match &record.body {
                        RecordBody::List { name } => name.clone(),
                        _ => return Err(Error::Invalid("list resolution failed".into())),
                    },
                ),
                [] if list_id.is_none() => {
                    let list = create(RecordBody::List {
                        name: list_name.trim().into(),
                    });
                    let id = list.command.id();
                    plan.commands.push(list);
                    (id, list_name.trim().to_owned())
                }
                [] => {
                    return Ok(clarify(
                        "That list ID is not available. Which list should I use?".into(),
                    ))
                }
                _ => {
                    return Ok(clarify(format!(
                        "There are multiple lists named {}. Which list ID do you mean: {}?",
                        preview(&list_name, 100),
                        matches
                            .iter()
                            .take(8)
                            .map(|record| record.id.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )))
                }
            };
            plan.reply = format!("Added {} to {}.", preview(&text, 200), preview(&name, 100));
            plan.commands.push(create(RecordBody::ListItem {
                list_id: parent,
                text,
                completed: false,
            }));
        }
        Intent::ShowList { list_name, list_id } => {
            let matches = matching_lists(records, &list_name, list_id)?;
            let parent = match matches.as_slice() {
                [record] => *record,
                [] => {
                    return Ok(clarify(
                        "I could not find that list. Which list should I show?".into(),
                    ))
                }
                _ => {
                    return Ok(clarify(format!(
                        "There are multiple matching lists. Which list ID do you mean: {}?",
                        matches
                            .iter()
                            .take(8)
                            .map(|record| record.id.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )))
                }
            };
            let items = records
                .iter()
                .filter(|record| !record.deleted && record.body.list_id() == Some(parent.id))
                .collect::<Vec<_>>();
            let labels = items
                .iter()
                .take(20)
                .map(|record| match &record.body {
                    RecordBody::ListItem {
                        text, completed, ..
                    } => format!(
                        "{}{}",
                        if *completed { "done: " } else { "" },
                        preview(text, 60)
                    ),
                    _ => String::new(),
                })
                .collect::<Vec<_>>();
            let name = match &parent.body {
                RecordBody::List { name } => name,
                _ => return Err(Error::Invalid("list resolution failed".into())),
            };
            plan.reply = if labels.is_empty() {
                format!("{} is empty.", preview(name, 100))
            } else {
                format!(
                    "{}: {}.{}",
                    preview(name, 100),
                    labels.join(", "),
                    if items.len() > 20 {
                        " Showing the first twenty items."
                    } else {
                        ""
                    }
                )
            };
            plan.status = TellStatus::Answer;
            plan.records.push(parent.clone());
            plan.records.extend(items.into_iter().take(49).cloned());
        }
        Intent::SaveNote { text } => {
            plan.commands.push(create(RecordBody::Note { text }));
            plan.reply = "Saved your note.".into();
        }
        Intent::SaveTask { title, due_at } => {
            plan.commands.push(create(RecordBody::Task {
                title,
                due_at,
                completed: false,
            }));
            plan.reply = "Saved your task. No reminder delivery has been scheduled.".into();
        }
        Intent::EditListItem {
            id,
            expected_revision,
            text,
        } => {
            let original = records
                .iter()
                .find(|record| record.id == id && !record.deleted)
                .ok_or(Error::NotFound(id))?;
            let RecordBody::ListItem {
                list_id, completed, ..
            } = &original.body
            else {
                return Ok(clarify(
                    "That ID is not a list item. Which item should I edit?".into(),
                ));
            };
            bounded_text(&text, 1_024)?;
            let body = RecordBody::ListItem {
                list_id: *list_id,
                text,
                completed: *completed,
            };
            if original.revision != expected_revision {
                return Err(Error::RevisionConflict {
                    expected: expected_revision,
                    actual: original.revision,
                });
            }
            if original.body == body {
                return Ok(clarify("That edit would leave the record unchanged. Which field and value should I change?".into()));
            }
            plan.commands.push(command(Command::Replace {
                id,
                expected_revision,
                body,
            }));
            plan.reply = "Updated the list item.".into();
        }
        Intent::SetTaskCompleted {
            id,
            expected_revision,
            completed,
        } => {
            let original = records
                .iter()
                .find(|record| record.id == id && !record.deleted)
                .ok_or(Error::NotFound(id))?;
            let RecordBody::Task { title, due_at, .. } = &original.body else {
                return Ok(clarify(
                    "That ID is not a task. Which task should I change?".into(),
                ));
            };
            if original.revision != expected_revision {
                return Err(Error::RevisionConflict {
                    expected: expected_revision,
                    actual: original.revision,
                });
            }
            let body = RecordBody::Task {
                title: title.clone(),
                due_at: *due_at,
                completed,
            };
            if original.body == body {
                return Ok(clarify("That task already has the requested completion state. Which task should I change?".into()));
            }
            plan.commands.push(command(Command::Replace {
                id,
                expected_revision,
                body,
            }));
            plan.reply = if completed {
                "Marked the task complete."
            } else {
                "Marked the task incomplete."
            }
            .into();
        }
        Intent::DeleteRecord {
            id,
            expected_revision,
        } => {
            plan.commands.push(command(Command::Delete {
                id,
                expected_revision,
            }));
            plan.reply = "Deleted the record.".into();
        }
        Intent::Clarify { question } => {
            bounded_text(&question, 2_048)?;
            return Ok(clarify(question));
        }
    }
    plan.validate()?;
    Ok(plan)
}
