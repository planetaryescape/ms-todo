//! What a move sends and what it checks (S14): the copy's POST body, built
//! from the source as Graph gave it, and the fields of a task compared
//! between the source and the copy.
//!
//! One POST carries the fields, the checklist items (checked or not, with
//! when they were checked), the one linked resource Graph allows and our
//! extension with the move's own `opId` and `originalCreatedAt`: S14 found
//! all of them kept, so everything but the attachments is created at once
//! and attributable by its `opId`.

use ms_todo_core::{DATE_FORMAT, completion_date, local_date_time, local_due_date};
use serde_json::{Map, Value, json};

use crate::task_fields::{DATE_ONLY, creatable_fields, graph_date, midnight};
use crate::task_writes::our_extension;
use ms_todo_store::Entity;

/// Where `createdDateTime` is kept: Graph stamps the copy with the time of
/// the move (S14).
pub(crate) const ORIGINAL_CREATED_AT: &str = "originalCreatedAt";

const CHECKLIST_FIELDS: [&str; 4] = [
    "displayName",
    "isChecked",
    "checkedDateTime",
    "createdDateTime",
];
const LINK_FIELDS: [&str; 4] = ["webUrl", "applicationName", "displayName", "externalId"];

/// The POST that makes the copy of `source` (Graph's JSON, as a GET gives
/// it) with our extension `extension`, as operation `op_id`.
pub(crate) fn copy_body(source: &Entity, extension: Option<&Value>, op_id: &str) -> Value {
    let mut body = creatable_fields(source);
    // A recurring task's dates are written in its recurrence's zone:
    // written in another, Graph moved the copy's due date and the series'
    // start a day on (S14).
    let recurrence_zone = source
        .get("recurrence")
        .and_then(|recurrence| recurrence["range"]["recurrenceTimeZone"].as_str());
    if let Some(zone) = recurrence_zone {
        for key in DATE_ONLY {
            if let Some(day) = graph_date(source, key, local_due_date) {
                body.insert(key.to_owned(), midnight(day, zone));
            }
        }
    }
    let checklist = children(source, "checklistItems", &CHECKLIST_FIELDS);
    if !checklist.is_empty() {
        body.insert("checklistItems".into(), Value::Array(checklist));
    }
    let links = children(source, "linkedResources", &LINK_FIELDS);
    if !links.is_empty() {
        body.insert("linkedResources".into(), Value::Array(links));
    }
    let mut ours = our_extension(op_id, extension);
    let original = original_created_at(source, extension);
    if let (Value::Object(fields), Some(original)) = (&mut ours, original) {
        fields.insert(ORIGINAL_CREATED_AT.into(), json!(original));
    }
    body.insert("extensions".into(), json!([ours]));
    Value::Object(body)
}

/// When the task was first made: kept from an earlier move, else Graph's.
pub(crate) fn original_created_at(source: &Entity, extension: Option<&Value>) -> Option<String> {
    extension
        .and_then(|extension| extension.get(ORIGINAL_CREATED_AT))
        .or_else(|| source.get("createdDateTime"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The items of the child collection `key` with only `fields`, as a POST
/// takes them.
fn children(source: &Entity, key: &str, fields: &[&str]) -> Vec<Value> {
    source
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    Value::Object(
                        fields
                            .iter()
                            .filter_map(|&field| {
                                let value = item.get(field).filter(|value| !value.is_null())?;
                                Some((field.to_owned(), value.clone()))
                            })
                            .collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A task's content as a move promises to keep it: every field a copy
/// can carry, dates as days, children by what they hold rather than
/// their IDs, and our extension without what each copy sets anew.
pub(crate) fn comparable(task: &Entity, extension: Option<&Value>) -> Map<String, Value> {
    let mut fields = Map::new();
    for key in [
        "title",
        "importance",
        "status",
        "isReminderOn",
        "categories",
        "recurrence",
    ] {
        fields.insert(key.into(), task.get(key).cloned().unwrap_or(Value::Null));
    }
    let body = task.get("body").unwrap_or(&Value::Null);
    fields.insert("body".into(), json!([body["content"], body["contentType"]]));
    let day =
        |key, read| graph_date(task, key, read).map(|date| date.format(DATE_FORMAT).to_string());
    for key in DATE_ONLY {
        fields.insert(key.into(), json!(day(key, local_due_date)));
    }
    fields.insert(
        "completedDateTime".into(),
        json!(day("completedDateTime", completion_date)),
    );
    let reminder = task.get("reminderDateTime").and_then(|value| {
        let at = local_date_time(
            value.get("dateTime")?.as_str()?,
            value.get("timeZone")?.as_str()?,
        )?;
        Some(at.format("%Y-%m-%dT%H:%M").to_string())
    });
    fields.insert("reminderDateTime".into(), json!(reminder));
    // What an item holds; Graph keeps when it was made to the second only.
    let checklist: Vec<Value> = children(task, "checklistItems", &["displayName", "isChecked"]);
    fields.insert("checklistItems".into(), Value::Array(checklist));
    fields.insert(
        "linkedResources".into(),
        Value::Array(children(task, "linkedResources", &LINK_FIELDS)),
    );
    let extension: Map<String, Value> = extension
        .and_then(Value::as_object)
        .map(|all| {
            all.iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "id" | "extensionName" | "opId" | ORIGINAL_CREATED_AT
                    ) && !key.contains("@odata")
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    fields.insert("extension".into(), Value::Object(extension));
    fields
}

/// The fields of `expected` that `actual` doesn't hold, by name.
pub(crate) fn differences(
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
) -> Vec<String> {
    expected
        .iter()
        .filter(|(key, value)| actual.get(key.as_str()) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    fn source() -> Entity {
        entity(json!({
            "id": "T1",
            "@odata.etag": "W/\"1\"",
            "title": "Renew passport",
            "status": "notStarted",
            "importance": "high",
            "isReminderOn": false,
            "hasAttachments": true,
            "categories": ["Admin"],
            "createdDateTime": "2026-09-24T10:00:00.1234567Z",
            "lastModifiedDateTime": "2026-09-24T11:00:00Z",
            "body": { "content": "notes", "contentType": "text" },
            // Midnight London, as a GET gives it.
            "dueDateTime": { "dateTime": "2026-09-27T23:00:00.0000000", "timeZone": "UTC" },
            "checklistItems": [
                { "id": "c1", "displayName": "one", "isChecked": true,
                  "checkedDateTime": "2026-09-24T12:00:00Z", "createdDateTime": "2026-09-24T10:00:00Z" }
            ],
            "linkedResources": [
                { "id": "r1", "webUrl": "https://example.com", "applicationName": "app",
                  "displayName": "link", "externalId": "x" }
            ]
        }))
    }

    #[test]
    fn the_copy_carries_every_field_and_child_but_no_ids_and_our_op_id() {
        let extension = json!({
            "id": "microsoft.graph.openTypeExtension.com.planetaryescape.mstodo",
            "extensionName": "com.planetaryescape.mstodo",
            "opId": "the-add",
            "assignee": "Sam"
        });
        let body = copy_body(&source(), Some(&extension), "the-move");
        for key in [
            "id",
            "@odata.etag",
            "hasAttachments",
            "createdDateTime",
            "lastModifiedDateTime",
        ] {
            assert!(body.get(key).is_none(), "{key} is Graph's, not sent");
        }
        assert_eq!(body["title"], "Renew passport");
        assert_eq!(body["categories"], json!(["Admin"]));
        assert_eq!(
            body["checklistItems"],
            json!([{ "displayName": "one", "isChecked": true,
                     "checkedDateTime": "2026-09-24T12:00:00Z", "createdDateTime": "2026-09-24T10:00:00Z" }])
        );
        assert_eq!(body["linkedResources"][0].get("id"), None);
        let ours = &body["extensions"][0];
        assert_eq!(ours["opId"], "the-move", "never the source's");
        assert_eq!(ours["assignee"], "Sam");
        assert_eq!(ours[ORIGINAL_CREATED_AT], "2026-09-24T10:00:00.1234567Z");
        // A one-off due date goes back as its local day, so Graph keeps it.
        let due = &body["dueDateTime"]["dateTime"];
        assert!(
            due.as_str().is_some_and(|due| due.ends_with("T00:00:00")),
            "{due}"
        );
    }

    #[test]
    fn a_task_moved_again_keeps_when_it_was_first_made() {
        let extension = json!({ "opId": "m1", ORIGINAL_CREATED_AT: "2020-01-01T00:00:00Z" });
        let body = copy_body(&source(), Some(&extension), "m2");
        assert_eq!(
            body["extensions"][0][ORIGINAL_CREATED_AT],
            "2020-01-01T00:00:00Z"
        );
    }

    #[test]
    fn a_recurring_task_s_dates_are_written_in_its_recurrence_zone() {
        let mut recurring = source();
        recurring.insert(
            "recurrence".into(),
            json!({ "pattern": { "type": "daily", "interval": 1 },
                    "range": { "type": "noEnd", "startDate": "2026-09-28", "recurrenceTimeZone": "UTC" } }),
        );
        let body = copy_body(&recurring, None, "m");
        assert_eq!(
            body["dueDateTime"],
            json!({ "dateTime": "2026-09-28T00:00:00", "timeZone": "UTC" })
        );
        assert_eq!(body["recurrence"], recurring["recurrence"]);
    }

    #[test]
    fn a_copy_differs_only_in_what_it_holds_not_its_ids_or_stamps() {
        let original = source();
        let mut copy = original.clone();
        copy.insert("id".into(), json!("C1"));
        copy.insert("createdDateTime".into(), json!("2026-09-25T00:00:00Z"));
        copy.insert(
            "checklistItems".into(),
            json!([{ "id": "other", "displayName": "one", "isChecked": true,
                     "createdDateTime": "2026-09-24T10:00:00Z" }]),
        );
        copy.insert(
            "dueDateTime".into(),
            json!({ "dateTime": "2026-09-28T00:00:00.0000000", "timeZone": "Europe/London" }),
        );
        let theirs = json!({ "opId": "the-add", "assignee": "Sam" });
        let ours = json!({ "opId": "the-move", "assignee": "Sam", ORIGINAL_CREATED_AT: "x" });
        let expected = comparable(&original, Some(&theirs));
        assert!(differences(&expected, &comparable(&copy, Some(&ours))).is_empty());

        copy.insert("checklistItems".into(), json!([]));
        copy.insert("importance".into(), json!("low"));
        let lost = json!({ "opId": "the-move" });
        assert_eq!(
            differences(&expected, &comparable(&copy, Some(&lost))),
            ["checklistItems", "extension", "importance"]
        );
    }
}
