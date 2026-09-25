//! Outlook categories (`/me/outlook/masterCategories`). A create is the
//! one POST here: Graph refuses a second category of the same name (S7),
//! so after a lost answer the categories are read again and one of that
//! name counts as made.

use ms_todo_core::ErrorKind;
use ms_todo_graph::GraphError;
use ms_todo_protocol::{CategoryChange, ErrorPayload, ResponseData, TaskAction};
use ms_todo_store::{Entity, OutboxRow};
use serde_json::{Map, Value, json};

use super::{applied, clean, invalid, plan, record, text, unknown_change};
use crate::handlers::{State, error_payload, graph_error};
use crate::undo::conflict;

pub(crate) async fn list_categories(state: &State) -> Result<ResponseData, ErrorPayload> {
    Ok(ResponseData::Categories {
        items: categories(state).await?,
    })
}

pub(crate) async fn change_category(
    state: &State,
    change: CategoryChange,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let categories = categories(state).await?;
    let (action, target, body) = match &change {
        CategoryChange::Create { name, color } => {
            let name = name.trim();
            if name.is_empty() {
                return Err(invalid("a category's name can't be empty".into()));
            }
            if let Some(found) = by_name(&categories, name) {
                return Err(invalid(format!(
                    "a category called {:?} exists already (names are unique ignoring case)",
                    text(found, "displayName")
                )));
            }
            let mut body = json!({ "displayName": name });
            if let Some(color) = color {
                body["color"] = json!(check_color(color)?);
            }
            let target = json!({ "displayName": name });
            (TaskAction::CategoryCreate, target, body)
        }
        CategoryChange::Recolor { category, color } => {
            let found = find_category(&categories, category)?;
            let body = json!({ "color": check_color(color)? });
            (
                TaskAction::CategoryRecolor,
                Value::Object(found.clone()),
                body,
            )
        }
        CategoryChange::Delete { category } => {
            let found = find_category(&categories, category)?;
            (
                TaskAction::CategoryDelete,
                Value::Object(found.clone()),
                Value::Null,
            )
        }
        CategoryChange::Unknown => return Err(unknown_change()),
    };
    if dry_run {
        return Ok(plan(action, target, body));
    }
    let before = target.as_object().cloned().unwrap_or_default();
    let after = match action {
        TaskAction::CategoryCreate => create_category(state, &body).await?,
        TaskAction::CategoryRecolor => {
            let id = text(&before, "id");
            clean(
                state
                    .graph
                    .update_category(&id, &body)
                    .await
                    .map_err(graph_error)?,
            )
        }
        _ => {
            state
                .graph
                .delete_category(&text(&before, "id"))
                .await
                .map_err(graph_error)?;
            before.clone()
        }
    };
    let rollback = if action == TaskAction::CategoryCreate {
        Map::new()
    } else {
        before
    };
    let payload = json!({ "kind": "category", "after": after });
    record(
        state,
        &op_id,
        None,
        &text(&after, "id"),
        action,
        &payload,
        &rollback,
    )
    .await;
    Ok(applied(op_id, action, after, None))
}

/// POST a category. A lost answer is settled by reading the categories
/// again: one with the name was made, since names are unique (S7).
async fn create_category(state: &State, body: &Value) -> Result<Entity, ErrorPayload> {
    match state.graph.create_category(body).await {
        Ok(created) => Ok(clean(created)),
        Err(error @ GraphError::OutcomeUnknown(_)) => {
            let name = body["displayName"].as_str().unwrap_or_default();
            let again = categories(state).await.map_err(|_| graph_error(error))?;
            by_name(&again, name).cloned().ok_or_else(|| {
                error_payload(
                    ErrorKind::Api,
                    format!(
                        "Microsoft To Do didn't answer creating {name:?}, and it isn't among \
                         your categories, so it wasn't made; try again"
                    ),
                )
            })
        }
        Err(error) => Err(graph_error(error)),
    }
}

pub(super) async fn undo(
    state: &State,
    target: &str,
    op: &OutboxRow,
    before: Entity,
    op_id: &str,
) -> Result<ResponseData, ErrorPayload> {
    let categories = categories(state).await?;
    let after = op.payload["after"].as_object().cloned().unwrap_or_default();
    let id = text(&after, "id");
    let now = categories
        .iter()
        .find(|category| text(category, "id") == id);
    let (action, result, rollback) = match op.action.as_str() {
        "category_create" | "category_recolor" => {
            let Some(now) = now else {
                return Err(error_payload(
                    ErrorKind::NotFound,
                    format!(
                        "the category {:?} is gone already",
                        text(&after, "displayName")
                    ),
                ));
            };
            if now.get("color") != after.get("color") {
                return Err(conflict(format!(
                    "the category {:?} was recoloured since; undo would overwrite that",
                    text(now, "displayName")
                )));
            }
            if op.action == "category_create" {
                state
                    .graph
                    .delete_category(&id)
                    .await
                    .map_err(graph_error)?;
                (TaskAction::CategoryDelete, now.clone(), now.clone())
            } else {
                let body =
                    json!({ "color": before.get("color").cloned().unwrap_or(json!("none")) });
                let changed = state
                    .graph
                    .update_category(&id, &body)
                    .await
                    .map_err(graph_error)?;
                (TaskAction::CategoryRecolor, clean(changed), now.clone())
            }
        }
        _ => {
            let name = text(&before, "displayName");
            if let Some(found) = by_name(&categories, &name) {
                return Err(conflict(format!(
                    "a category called {:?} was made since; undo would make a second",
                    text(found, "displayName")
                )));
            }
            let mut body = json!({ "displayName": name });
            if let Some(color) = before.get("color") {
                body["color"] = color.clone();
            }
            let made = create_category(state, &body).await?;
            (TaskAction::CategoryCreate, made, Map::new())
        }
    };
    let payload = json!({ "kind": "category", "after": result });
    let entity = text(&result, "id");
    record(
        state,
        op_id,
        Some(target),
        &entity,
        action,
        &payload,
        &rollback,
    )
    .await;
    Ok(applied(op_id.to_owned(), action, result, Some(target)))
}

async fn categories(state: &State) -> Result<Vec<Entity>, ErrorPayload> {
    Ok(state
        .graph
        .list_categories()
        .await
        .map_err(graph_error)?
        .into_iter()
        .map(clean)
        .collect())
}

fn find_category<'a>(categories: &'a [Entity], wanted: &str) -> Result<&'a Entity, ErrorPayload> {
    categories
        .iter()
        .find(|category| text(category, "id") == wanted)
        .or_else(|| by_name(categories, wanted))
        .ok_or_else(|| {
            error_payload(
                ErrorKind::NotFound,
                format!(
                    "no category is called or has the ID {wanted:?}; see `ms-todo categories list`"
                ),
            )
        })
}

fn by_name<'a>(categories: &'a [Entity], name: &str) -> Option<&'a Entity> {
    categories
        .iter()
        .find(|category| text(category, "displayName").eq_ignore_ascii_case(name.trim()))
}

/// Outlook's colours: `preset0` to `preset24`, or `none`.
fn check_color(color: &str) -> Result<String, ErrorPayload> {
    let color = color.trim().to_lowercase();
    let preset = color
        .strip_prefix("preset")
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| number <= 24);
    if preset || color == "none" {
        Ok(color)
    } else {
        Err(invalid(format!(
            "{color:?} isn't a colour: use preset0 to preset24, or none"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    #[test]
    fn a_category_is_found_by_id_or_by_name_ignoring_case() {
        let categories = vec![
            entity(json!({ "id": "c1", "displayName": "Errands", "color": "preset3" })),
            entity(json!({ "id": "c2", "displayName": "Bills", "color": "none" })),
        ];
        assert_eq!(
            text(find_category(&categories, "c2").expect("id"), "id"),
            "c2"
        );
        assert_eq!(
            text(find_category(&categories, "errands").expect("name"), "id"),
            "c1"
        );
        assert!(find_category(&categories, "Gym").is_err());
        assert!(check_color("preset25").is_err());
        assert_eq!(check_color(" PRESET24 ").expect("colour"), "preset24");
    }
}
