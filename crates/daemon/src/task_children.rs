//! `steps add|edit|check|uncheck|delete` and `links add|edit|delete`
//! (docs/blueprint/07-cli.md): writes to one task's steps (Graph's
//! `checklistItems`) and its link (`linkedResources`). Each step or link
//! written is one `child` outbox operation, all under the command's
//! `op_id`, applied to the cached task at once like any write (D-055).
//!
//! A step is named by its number from 1 (as `steps list` shows them, in
//! Graph's order, which is the order they were added), its ID, or its
//! exact text. Graph has no way to reorder steps (S15), so there's no
//! `steps order`.

use ms_todo_core::ErrorKind;
use ms_todo_core::links::storable;
use ms_todo_protocol::{
    ErrorPayload, LinkEdit, NewLink, Plan, PlannedTask, ResponseData, TaskAction, TaskChange,
};
use ms_todo_store::{
    ChildVerb, Entity, LINKS, LOCAL_CHILD_PREFIX, LocalChange, NewOp, OpKind, STEPS, child_payload,
    children,
};
use serde_json::{Map, Value, json};

use crate::handlers::{State, error_payload};
use crate::outbox::op_id_for;
use crate::task_resolution::Target;
use crate::task_writes::{Targets, action_name, queue, resolve};

/// `applicationName`, which Graph requires on a link (S15), when none is
/// given.
pub(crate) const DEFAULT_APP: &str = "ms-todo";

/// Sent with every step PATCH, since Graph unchecks a step whose PATCH
/// leaves it out (S1); it's carried, not changed.
pub(crate) const CARRIED_ON_STEPS: &[&str] = &["isChecked"];

/// The fields a create of each kind of child sends, from a copy of one:
/// what undo re-creates a deleted one from.
pub(crate) const STEP_FIELDS: &[&str] = &["displayName", "isChecked", "checkedDateTime"];
pub(crate) const LINK_FIELDS: &[&str] = &["webUrl", "applicationName", "displayName", "externalId"];

/// One child write, before it's an outbox operation.
#[derive(Debug)]
pub(crate) struct ChildWrite {
    pub collection: &'static str,
    pub verb: ChildVerb,
    pub id: String,
    pub body: Value,
    /// Fields of `body` sent only because Graph resets what a PATCH
    /// leaves out, not changed by it.
    pub carried: &'static [&'static str],
    pub action: TaskAction,
    /// More of the payload, beside the child's: an attachment's `file`
    /// to read, or `no_undo` on its delete (`crate::attachments`).
    pub extra: Map<String, Value>,
}

impl ChildWrite {
    pub fn create(collection: &'static str, body: Value, action: TaskAction) -> Self {
        Self {
            collection,
            verb: ChildVerb::Create,
            id: format!("{LOCAL_CHILD_PREFIX}{}", uuid::Uuid::new_v4()),
            body,
            carried: &[],
            action,
            extra: Map::new(),
        }
    }

    pub fn update(collection: &'static str, id: &str, body: Value, action: TaskAction) -> Self {
        Self {
            collection,
            verb: ChildVerb::Update,
            id: id.to_owned(),
            body,
            carried: &[],
            action,
            extra: Map::new(),
        }
    }

    pub fn delete(collection: &'static str, id: &str, action: TaskAction) -> Self {
        Self {
            collection,
            verb: ChildVerb::Delete,
            id: id.to_owned(),
            body: json!({}),
            carried: &[],
            action,
            extra: Map::new(),
        }
    }

    pub fn payload(&self) -> Value {
        let mut payload = child_payload(
            self.collection,
            self.verb,
            &self.id,
            self.body.clone(),
            self.carried,
        );
        for (key, value) in &self.extra {
            payload[key] = value.clone();
        }
        payload
    }

    pub fn into_op(self, op_id: String, row: &ms_todo_store::TaskRow) -> NewOp {
        NewOp {
            op_id,
            entity_local_id: row.local_id.clone(),
            list_local_id: row.list_local_id.clone(),
            op: OpKind::Child,
            action: action_name(self.action).to_owned(),
            payload: self.payload(),
            change: LocalChange::Child,
        }
    }
}

pub(crate) async fn change(
    state: &State,
    targets: &Targets<'_>,
    change: TaskChange,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let writes = |raw: &Entity| plan(raw, change);
    change_children(state, targets, "steps and links", dry_run, op_id, writes).await
}

/// Resolve the one task `targets` names, plan its child writes with
/// `plan`, then answer the plan for a dry run or queue them. `what`
/// names the children, for the errors.
pub(crate) async fn change_children(
    state: &State,
    targets: &Targets<'_>,
    what: &str,
    dry_run: bool,
    op_id: String,
    plan: impl FnOnce(&Entity) -> Result<(TaskAction, Vec<ChildWrite>), ErrorPayload>,
) -> Result<ResponseData, ErrorPayload> {
    if targets.select.is_some() {
        return Err(invalid(format!(
            "name the task; --overdue and --due-before don't pick a task for {what}"
        )));
    }
    let target = match resolve(state, targets).await?.as_slice() {
        [one] => one.clone(),
        _ => {
            return Err(invalid(format!(
                "{what} change one task at a time; name one task"
            )));
        }
    };
    let (action, writes) = plan(&target.row.raw)?;
    if dry_run {
        return Ok(ResponseData::Plan(Plan {
            action,
            list: None,
            targets: vec![planned(&target)],
            lists: Vec::new(),
            changes: Value::Array(writes.iter().map(ChildWrite::payload).collect()),
        }));
    }
    let ops = writes
        .into_iter()
        .enumerate()
        .map(|(index, write)| write.into_op(op_id_for(&op_id, index), &target.row))
        .collect();
    queue(state, &op_id, None, ops, action).await
}

fn planned(target: &Target) -> PlannedTask {
    PlannedTask {
        id: target.local_id().to_owned(),
        title: target.title().to_owned(),
        list_id: target.list.local_id.clone(),
    }
}

/// The writes `change` makes to the task `raw`, and what the command is
/// called. A step or link already as asked is left out, so a repeat
/// queues nothing.
pub(crate) fn plan(
    raw: &Entity,
    change: TaskChange,
) -> Result<(TaskAction, Vec<ChildWrite>), ErrorPayload> {
    Ok(match change {
        TaskChange::AddSteps { steps } => {
            let mut writes = Vec::new();
            for text in &steps {
                let text = step_text(text)?;
                let body = json!({ "displayName": text, "isChecked": false });
                writes.push(ChildWrite::create(STEPS, body, TaskAction::StepAdd));
            }
            if writes.is_empty() {
                return Err(invalid("give the text of at least one step".into()));
            }
            (TaskAction::StepAdd, writes)
        }
        TaskChange::EditStep { step, text } => {
            let text = step_text(&text)?;
            let found = find_steps(raw, std::slice::from_ref(&step))?;
            let item = &found[0];
            let writes = if item["displayName"] == text {
                Vec::new()
            } else {
                let checked = item["isChecked"].as_bool().unwrap_or(false);
                let mut rename = update(
                    STEPS,
                    item,
                    json!({ "displayName": text, "isChecked": checked }),
                    TaskAction::StepEdit,
                );
                rename.carried = CARRIED_ON_STEPS;
                vec![rename]
            };
            (TaskAction::StepEdit, writes)
        }
        TaskChange::CheckSteps { steps, checked } => {
            let action = if checked {
                TaskAction::StepCheck
            } else {
                TaskAction::StepUncheck
            };
            let writes = find_steps(raw, &steps)?
                .into_iter()
                .filter(|item| item["isChecked"].as_bool().unwrap_or(false) != checked)
                .map(|item| update(STEPS, &item, json!({ "isChecked": checked }), action))
                .collect();
            (action, writes)
        }
        TaskChange::DeleteSteps { steps } => {
            let writes = find_steps(raw, &steps)?
                .into_iter()
                .map(|item| delete(STEPS, &item, TaskAction::StepDelete))
                .collect();
            (TaskAction::StepDelete, writes)
        }
        TaskChange::AddLink(link) => (TaskAction::LinkAdd, vec![add_link(raw, link)?]),
        TaskChange::EditLink(edit) => (TaskAction::LinkEdit, edit_link(raw, edit)?),
        TaskChange::DeleteLink { link } => {
            let item = find_link(raw, link.as_deref())?;
            (
                TaskAction::LinkDelete,
                vec![delete(LINKS, &item, TaskAction::LinkDelete)],
            )
        }
        _ => {
            return Err(error_payload(
                ErrorKind::Internal,
                "not a change to steps or links".into(),
            ));
        }
    })
}

fn update(collection: &'static str, item: &Value, body: Value, action: TaskAction) -> ChildWrite {
    ChildWrite::update(collection, child_id(item), body, action)
}

fn delete(collection: &'static str, item: &Value, action: TaskAction) -> ChildWrite {
    ChildWrite::delete(collection, child_id(item), action)
}

fn child_id(item: &Value) -> &str {
    item["id"].as_str().unwrap_or_default()
}

fn step_text(text: &str) -> Result<&str, ErrorPayload> {
    let text = text.trim();
    if text.is_empty() {
        return Err(invalid("a step's text can't be empty".into()));
    }
    Ok(text)
}

/// The steps `names` name, each once, in the order named; every name must
/// match, or nothing is changed.
fn find_steps(raw: &Entity, names: &[String]) -> Result<Vec<Value>, ErrorPayload> {
    find_all(children(raw, STEPS), names, "step", "displayName")
}

/// The children `names` name among `items` (see [`find`]), each once, in
/// the order named; every name must match, or it's an error.
pub(crate) fn find_all(
    items: &[Value],
    names: &[String],
    noun: &str,
    text_key: &str,
) -> Result<Vec<Value>, ErrorPayload> {
    if names.is_empty() {
        return Err(invalid(format!("name at least one {noun}")));
    }
    let mut found: Vec<Value> = Vec::new();
    for name in names {
        let item = find(items, name, noun, text_key)?;
        if !found.iter().any(|seen| seen["id"] == item["id"]) {
            found.push(item.clone());
        }
    }
    Ok(found)
}

/// The child `name` names among `items`: by ID, then a number from 1,
/// then exact `text_key`. A text several share is an error listing them;
/// it never picks one.
fn find<'a>(
    items: &'a [Value],
    name: &str,
    noun: &str,
    text_key: &str,
) -> Result<&'a Value, ErrorPayload> {
    if let Some(item) = items.iter().find(|item| item["id"] == name) {
        return Ok(item);
    }
    if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()) {
        let number: usize = name.parse().unwrap_or(0);
        return number
            .checked_sub(1)
            .and_then(|at| items.get(at))
            .ok_or_else(|| {
                not_found(format!(
                    "the task has {} {noun}s, numbered from 1; there's no {noun} {name}",
                    items.len()
                ))
            });
    }
    let matching: Vec<(usize, &Value)> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item[text_key] == name)
        .collect();
    match matching.as_slice() {
        [(_, item)] => Ok(item),
        [] => Err(not_found(format!(
            "no {noun} of the task is {name:?}; name it by its number from 1, its ID or its \
             exact text (`ms-todo {noun}s list`)"
        ))),
        several => {
            let numbers: Vec<String> = several.iter().map(|(at, _)| (at + 1).to_string()).collect();
            Err(invalid(format!(
                "{} {noun}s are {name:?} ({}); name one by its number",
                several.len(),
                numbers.join(", ")
            )))
        }
    }
}

fn find_link(raw: &Entity, name: Option<&str>) -> Result<Value, ErrorPayload> {
    let links = children(raw, LINKS);
    match (name, links) {
        (_, []) => Err(not_found("the task has no link".into())),
        (None, [only]) => Ok(only.clone()),
        (None, _) => Err(invalid(
            "the task has several links; name one by its number or ID".into(),
        )),
        (Some(name), links) => find(links, name, "link", "webUrl").cloned(),
    }
}

/// A new link, refused when the task has one: Graph allows one per task
/// (S14), and says only "Linked Resource already exists".
fn add_link(raw: &Entity, link: NewLink) -> Result<ChildWrite, ErrorPayload> {
    if let Some(existing) = children(raw, LINKS).first() {
        let url = existing["webUrl"].as_str().unwrap_or_default();
        return Err(invalid(format!(
            "the task has a link already ({url}), and Microsoft To Do allows one per task; \
             change it with `ms-todo links edit`, or delete it first"
        )));
    }
    let mut body = Map::new();
    body.insert("webUrl".into(), json!(valid_url(&link.url)?));
    body.insert(
        "applicationName".into(),
        json!(non_empty(link.app).unwrap_or_else(|| DEFAULT_APP.to_owned())),
    );
    if let Some(name) = non_empty(link.name) {
        body.insert("displayName".into(), json!(name));
    }
    if let Some(id) = non_empty(link.external_id) {
        body.insert("externalId".into(), json!(id));
    }
    Ok(ChildWrite::create(
        LINKS,
        Value::Object(body),
        TaskAction::LinkAdd,
    ))
}

/// The fields of the link that change. Graph ignores a null or empty
/// value in a link's PATCH (S15), so a field can be set but not cleared.
fn edit_link(raw: &Entity, edit: LinkEdit) -> Result<Vec<ChildWrite>, ErrorPayload> {
    let item = find_link(raw, edit.link.as_deref())?;
    let url = edit.url.as_deref().map(valid_url).transpose()?;
    let fields = [
        ("webUrl", url.map(str::to_owned)),
        ("displayName", edit.name),
        ("applicationName", edit.app),
        ("externalId", edit.external_id),
    ];
    if fields.iter().all(|(_, value)| value.is_none()) {
        return Err(invalid(
            "say what to change: --url, --name, --app or --external-id".into(),
        ));
    }
    let mut body = Map::new();
    for (key, value) in fields {
        let Some(value) = value else { continue };
        if value.trim().is_empty() {
            return Err(invalid(format!(
                "Microsoft To Do can't clear a link's {key}; give a value, or delete the link \
                 and add it again"
            )));
        }
        if item[key] != value.as_str() {
            body.insert(key.to_owned(), json!(value));
        }
    }
    Ok(if body.is_empty() {
        Vec::new()
    } else {
        vec![update(
            LINKS,
            &item,
            Value::Object(body),
            TaskAction::LinkEdit,
        )]
    })
}

fn valid_url(url: &str) -> Result<&str, ErrorPayload> {
    let url = url.trim();
    storable(url).map_err(|why| invalid(format!("{url:?} can't be a link: {why}")))?;
    Ok(url)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) fn invalid(message: String) -> ErrorPayload {
    error_payload(ErrorKind::InvalidInput, message)
}

fn not_found(message: String) -> ErrorPayload {
    error_payload(ErrorKind::NotFound, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    fn painted() -> Entity {
        raw(json!({ "checklistItems": [
            { "id": "c1", "displayName": "Buy paint", "isChecked": false },
            { "id": "c2", "displayName": "Tape", "isChecked": true },
            { "id": "c3", "displayName": "Tape", "isChecked": false }
        ] }))
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_step_is_named_by_id_number_or_exact_text() {
        let task = painted();
        let found = find_steps(&task, &names(&["c3", "1", "Buy paint"])).expect("found");
        let ids: Vec<&Value> = found.iter().map(|step| &step["id"]).collect();
        assert_eq!(ids, [&json!("c3"), &json!("c1")], "each once, in order");
        let shared = find_steps(&task, &names(&["Tape"])).expect_err("ambiguous");
        assert_eq!(shared.kind, "invalid_input");
        assert!(shared.message.contains("(2, 3)"), "{}", shared.message);
        assert_eq!(
            find_steps(&task, &names(&["4"])).expect_err("none").kind,
            "not_found"
        );
        assert_eq!(
            find_steps(&task, &names(&["Paint"]))
                .expect_err("none")
                .kind,
            "not_found"
        );
    }

    #[test]
    fn every_step_patch_carries_is_checked() {
        let task = painted();
        let edit = TaskChange::EditStep {
            step: "2".into(),
            text: "Masking tape".into(),
        };
        let (_, writes) = plan(&task, edit).expect("plan");
        assert_eq!(
            writes[0].payload()["body"],
            json!({ "displayName": "Masking tape", "isChecked": true })
        );
        assert_eq!(writes[0].payload()["carried"], json!(["isChecked"]));
        let check = TaskChange::CheckSteps {
            steps: names(&["1", "2"]),
            checked: true,
        };
        let (action, writes) = plan(&task, check).expect("plan");
        assert_eq!(action, TaskAction::StepCheck);
        assert_eq!(writes.len(), 1, "step 2 is checked already");
        assert_eq!(writes[0].payload()["body"], json!({ "isChecked": true }));
        assert_eq!(
            writes[0].payload()["carried"],
            json!([]),
            "the change itself"
        );
    }

    #[test]
    fn steps_are_added_in_order_under_placeholder_ids() {
        let add = TaskChange::AddSteps {
            steps: names(&["Buy paint", " Tape "]),
        };
        let (_, writes) = plan(&raw(json!({})), add).expect("plan");
        let texts: Vec<&Value> = writes
            .iter()
            .map(|write| &write.body["displayName"])
            .collect();
        assert_eq!(texts, [&json!("Buy paint"), &json!("Tape")]);
        assert!(
            writes
                .iter()
                .all(|write| write.id.starts_with(LOCAL_CHILD_PREFIX))
        );
        let empty = TaskChange::AddSteps {
            steps: names(&[" "]),
        };
        assert!(plan(&raw(json!({})), empty).is_err());
    }

    #[test]
    fn a_second_link_is_refused_and_a_link_needs_a_url() {
        let link = |url: &str| {
            TaskChange::AddLink(NewLink {
                url: url.into(),
                ..NewLink::default()
            })
        };
        let (_, writes) = plan(&raw(json!({})), link("https://example.com/a")).expect("plan");
        assert_eq!(
            writes[0].body,
            json!({ "webUrl": "https://example.com/a", "applicationName": "ms-todo" })
        );
        let linked =
            raw(json!({ "linkedResources": [{ "id": "r1", "webUrl": "https://a.example" }] }));
        let refused = plan(&linked, link("https://example.com/b")).expect_err("one link");
        assert!(
            refused.message.contains("allows one per task"),
            "{}",
            refused.message
        );
        assert!(plan(&raw(json!({})), link("not a url")).is_err());
    }

    #[test]
    fn a_link_edit_sends_only_what_changes_and_never_clears() {
        let linked = raw(json!({ "linkedResources": [
            { "id": "r1", "webUrl": "https://a.example", "applicationName": "ms-todo" }
        ] }));
        let edit = |edit: LinkEdit| plan(&linked, TaskChange::EditLink(edit));
        let (_, writes) = edit(LinkEdit {
            url: Some("https://a.example".into()),
            name: Some("Spec".into()),
            ..LinkEdit::default()
        })
        .expect("plan");
        assert_eq!(writes[0].body, json!({ "displayName": "Spec" }));
        let (_, same) = edit(LinkEdit {
            url: Some("https://a.example".into()),
            ..LinkEdit::default()
        })
        .expect("plan");
        assert!(same.is_empty(), "nothing changes");
        assert!(
            edit(LinkEdit {
                name: Some(String::new()),
                ..LinkEdit::default()
            })
            .is_err()
        );
        assert!(edit(LinkEdit::default()).is_err(), "nothing said");
    }
}
