import { execFile } from "node:child_process";
import { accessSync, constants } from "node:fs";
import { delimiter, isAbsolute, join } from "node:path";
import { promisify } from "node:util";
import { z } from "zod";

const execFileAsync = promisify(execFile);

const taskSchema = z.object({
  id: z.string(),
  title: z.string(),
  status: z.string(),
  list: z.string().optional(),
  sync_state: z.string().optional(),
});
const collectionSchema = z.object({
  schema_version: z.literal(2),
  sync: z.object({ state: z.string(), generation: z.number() }),
  items: z.array(taskSchema),
});
const mutationSchema = z.object({
  schema_version: z.literal(2),
  action: z.string(),
  op_id: z.string(),
  items: z.array(z.object({ id: z.string() })).min(1),
});
const errorSchema = z.object({ error: z.object({ message: z.string() }) });
const execErrorSchema = z.object({
  stderr: z.string().optional(),
  killed: z.boolean().optional(),
  code: z.union([z.string(), z.number()]).optional(),
  message: z.string(),
});

export type Task = z.infer<typeof taskSchema>;
export type Collection = Pick<
  z.infer<typeof collectionSchema>,
  "sync" | "items"
>;

export class CliError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CliError";
  }
}

function executable(path: string): boolean {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export function findCli(
  preferredPath = "",
  pathEnv = process.env.PATH ?? "",
): string {
  if (preferredPath.trim()) {
    const path = preferredPath.trim();
    if (!isAbsolute(path) || !executable(path)) {
      throw new CliError(
        `The configured ms-todo CLI path is not an executable absolute path: ${path}`,
      );
    }
    return path;
  }
  const candidates = [
    "/opt/homebrew/bin/ms-todo",
    "/usr/local/bin/ms-todo",
    ...(process.env.HOME ? [join(process.env.HOME, ".local/bin/ms-todo")] : []),
    ...pathEnv
      .split(delimiter)
      .filter(Boolean)
      .map((dir) => join(dir, "ms-todo")),
  ];
  const found = candidates.find(executable);
  if (!found) {
    throw new CliError(
      "ms-todo is not installed or Raycast cannot find it. Install it with Homebrew, or set CLI Path in extension preferences.",
    );
  }
  return found;
}

function errorMessage(stderr: string, fallback: string): string {
  try {
    const parsed = errorSchema.safeParse(JSON.parse(stderr));
    if (parsed.success) return parsed.data.error.message;
  } catch {
    // Failures before CLI startup may not produce JSON.
  }
  return stderr.trim() || fallback;
}

function parseOutput<T>(
  stdout: string,
  schema: z.ZodType<T>,
  responseError: string,
): T {
  let data: unknown;
  try {
    data = JSON.parse(stdout);
  } catch {
    throw new CliError(
      "ms-todo returned invalid JSON. Check that the installed CLI is current.",
    );
  }
  if (!z.object({ schema_version: z.literal(2) }).safeParse(data).success) {
    throw new CliError(
      "ms-todo returned an unsupported JSON schema. Update the CLI.",
    );
  }
  const parsed = schema.safeParse(data);
  if (!parsed.success) throw new CliError(responseError);
  return parsed.data;
}

async function runCli<T>(
  args: string[],
  schema: z.ZodType<T>,
  responseError: string,
  preferredPath = "",
): Promise<T> {
  const path = findCli(preferredPath);
  try {
    const { stdout } = await execFileAsync(
      path,
      ["--format", "json", ...args],
      { timeout: 15_000, maxBuffer: 8 * 1024 * 1024, encoding: "utf8" },
    );
    return parseOutput(stdout, schema, responseError);
  } catch (error) {
    if (error instanceof CliError) throw error;
    const failure = execErrorSchema.safeParse(error);
    if (!failure.success) throw new CliError(String(error));
    if (failure.data.killed) {
      throw new CliError(
        "ms-todo did not respond within 15 seconds. Check the daemon with `ms-todo doctor`.",
      );
    }
    if (failure.data.code === "ENOENT") {
      throw new CliError(
        "The ms-todo CLI disappeared. Check CLI Path in extension preferences.",
      );
    }
    throw new CliError(
      errorMessage(failure.data.stderr ?? "", failure.data.message),
    );
  }
}

export async function searchTasks(
  query: string,
  preferredPath = "",
): Promise<Collection> {
  return runCli(
    ["search", query, "--status", "open"],
    collectionSchema,
    "ms-todo returned an incomplete task collection. Update the CLI.",
    preferredPath,
  );
}

export async function myDay(preferredPath = ""): Promise<Collection> {
  return runCli(
    ["myday", "list"],
    collectionSchema,
    "ms-todo returned an incomplete task collection. Update the CLI.",
    preferredPath,
  );
}

async function write(
  args: string[],
  action: string,
  preferredPath: string,
): Promise<void> {
  const confirmationError = `ms-todo did not confirm that the task was ${action === "add" ? "added" : "completed"}.`;
  const result = await runCli(
    args,
    mutationSchema,
    confirmationError,
    preferredPath,
  );
  if (result.action !== action) throw new CliError(confirmationError);
}

export async function addTask(text: string, preferredPath = ""): Promise<void> {
  await write(["tasks", "add", text, "--strict"], "add", preferredPath);
}

export async function completeTask(
  id: string,
  preferredPath = "",
): Promise<void> {
  await write(["tasks", "complete", id], "complete", preferredPath);
}
