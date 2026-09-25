import {
  Action,
  ActionPanel,
  Icon,
  List,
  getPreferenceValues,
  showToast,
  Toast,
} from "@raycast/api";
import { useCallback, useEffect, useState } from "react";
import { Collection, Task, completeTask, myDay, searchTasks } from "./cli";

type Mode = "search" | "my-day";

type Preferences = { cliPath?: string };

export function TaskList({ mode }: { mode: Mode }) {
  const { cliPath } = getPreferenceValues<Preferences>();
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<Collection>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    if (mode === "search" && !query.trim()) {
      setResult(undefined);
      setError(undefined);
      setLoading(false);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(
      () => {
        setLoading(true);
        const load =
          mode === "search"
            ? searchTasks(query.trim(), cliPath)
            : myDay(cliPath);
        void load
          .then((value) => {
            if (!cancelled) {
              setResult(value);
              setError(undefined);
            }
          })
          .catch((cause: unknown) => {
            if (!cancelled) {
              setResult(undefined);
              setError(cause instanceof Error ? cause.message : String(cause));
            }
          })
          .finally(() => {
            if (!cancelled) setLoading(false);
          });
      },
      mode === "search" ? 180 : 0,
    );
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [mode, query, cliPath, revision]);

  const finish = useCallback(
    async (task: Task) => {
      try {
        await completeTask(task.id, cliPath);
        await showToast({
          style: Toast.Style.Success,
          title: "Task completed",
          message: task.title,
        });
        setRevision((value) => value + 1);
      } catch (cause) {
        await showToast({
          style: Toast.Style.Failure,
          title: "Could not complete task",
          message: cause instanceof Error ? cause.message : String(cause),
        });
      }
    },
    [cliPath],
  );

  const emptyTitle =
    error ??
    (result?.sync.state === "initial"
      ? "Initial sync is still running"
      : mode === "search" && !query.trim()
        ? "Type to search cached tasks"
        : "No tasks found");
  const emptyDescription = error
    ? "Check the CLI Path preference, sign-in, and `ms-todo doctor`."
    : result?.sync.state === "initial"
      ? "Refresh after ms-todo finishes its first sync."
      : undefined;

  return (
    <List
      filtering={false}
      isLoading={loading}
      searchBarPlaceholder={
        mode === "search" ? "Search task titles and notes" : undefined
      }
      onSearchTextChange={mode === "search" ? setQuery : undefined}
      throttle={mode === "search"}
    >
      {result?.sync.state !== "initial" &&
        result?.items.map((task) => (
          <List.Item
            key={task.id}
            title={task.title}
            subtitle={task.list}
            icon={task.status === "completed" ? Icon.CheckCircle : Icon.Circle}
            accessories={
              task.sync_state && task.sync_state !== "synced"
                ? [{ tag: task.sync_state }]
                : undefined
            }
            actions={
              <ActionPanel>
                {task.status !== "completed" && (
                  <Action
                    title="Complete Task"
                    icon={Icon.CheckCircle}
                    onAction={() => void finish(task)}
                  />
                )}
                <Action
                  title="Refresh"
                  icon={Icon.ArrowClockwise}
                  onAction={() => setRevision((value) => value + 1)}
                />
              </ActionPanel>
            }
          />
        ))}
      {!loading &&
        (!result?.items.length || result.sync.state === "initial") && (
          <List.EmptyView
            title={emptyTitle}
            description={emptyDescription}
            actions={
              <ActionPanel>
                <Action
                  title="Refresh"
                  icon={Icon.ArrowClockwise}
                  onAction={() => setRevision((value) => value + 1)}
                />
              </ActionPanel>
            }
          />
        )}
    </List>
  );
}
