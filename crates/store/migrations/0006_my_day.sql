-- Rung 7's My Day (docs/blueprint/05-custom-features.md#my-day). A task's
-- My Day is `myDay` in our extension, so it needs no column: the index
-- makes the My Day view and the rollover's search as quick as the other
-- smart views.
--
-- settings holds what the daemon remembers between runs: the day the last
-- rollover ran for (`my_day.last_rollover`), and the tasks it took out of
-- My Day still open (`my_day.left_over`), which My Day then suggests.

CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE INDEX tasks_by_my_day ON tasks(json_extract(extension_json, '$.myDay'));
