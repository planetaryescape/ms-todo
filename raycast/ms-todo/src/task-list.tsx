import {
  Action,
  ActionPanel,
  Icon,
  List,
  getPreferenceValues,
  showToast,
  Toast,
} from "@raycast/api";
import { useCallback, useEffect, useRef, useState } from "react";
import { Collection, Task, completeTask, myDay, searchTasks } from "./cli";

type Mode = "search" | "my-day";

type Preferences = { cliPath?: string };

export function TaskList({ mode }: { mode: Mode }) {
  const { cliPath } = getPreferenceValues<Preferences>();
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<{
    query: string;
    collection: Collection;
  }>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const completing = useRef(new Set<string>());

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
              setResult({ query: query.trim(), collection: value });
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
      if (completing.current.has(task.id)) return;
      completing.current.add(task.id);
      try {
        await completeTask(task.id, cliPath);
        await showToast({
          style: Toast.Style.Success,
          title: "Task completed",
          message: task.title,
        });
        setResult(undefined);
        setRevision((value) => value + 1);
      } catch (cause) {
        await showToast({
          style: Toast.Style.Failure,
          title: "Could not complete task",
          message: cause instanceof Error ? cause.message : String(cause),
        });
      } finally {
        completing.current.delete(task.id);
      }
    },
    [cliPath],
  );

  const visibleResult =
    result?.query === query.trim() ? result.collection : undefined;
  const awaitingResult =
    mode === "search" && !!query.trim() && !visibleResult && !error;

  const emptyTitle =
    error ??
    (visibleResult?.sync.state === "initial"
      ? "Initial sync is still running"
      : mode === "search" && !query.trim()
        ? "Type to search cached tasks"
        : "No tasks found");
  const emptyDescription = error
    ? "Check the CLI Path preference, sign-in, and `ms-todo doctor`."
    : visibleResult?.sync.state === "initial"
      ? "Refresh after ms-todo finishes its first sync."
      : undefined;

  return (
    <List
      filtering={false}
      isLoading={loading || awaitingResult}
      searchBarPlaceholder={
        mode === "search" ? "Search task titles and notes" : undefined
      }
      onSearchTextChange={
        mode === "search"
          ? (text) => {
              setQuery(text);
              setError(undefined);
            }
          : undefined
      }
    >
      {visibleResult?.sync.state !== "initial" &&
        visibleResult?.items.map((task) => (
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
        !awaitingResult &&
        (!visibleResult?.items.length ||
          visibleResult.sync.state === "initial") && (
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
