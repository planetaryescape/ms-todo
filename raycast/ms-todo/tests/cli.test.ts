import assert from "node:assert/strict";
import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  chmodSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "node:test";
import { z } from "zod";
import {
  addTask,
  CliError,
  completeTask,
  findCli,
  myDay,
  searchTasks,
} from "../src/cli";

const directories: string[] = [];
function fakeCli(output: string, exit = 0) {
  const dir = mkdtempSync(join(tmpdir(), "ms-todo-raycast-"));
  directories.push(dir);
  const path = join(dir, "ms-todo");
  const script = `#!/usr/bin/env node\nconst fs=require('node:fs');\nfs.writeFileSync(${JSON.stringify(join(dir, "args.json"))},JSON.stringify(process.argv.slice(2)));\nprocess.${exit ? "stderr" : "stdout"}.write(${JSON.stringify(output)});\nprocess.exit(${exit});\n`;
  writeFileSync(path, script);
  chmodSync(path, 0o755);
  return {
    path,
    args: () =>
      z
        .array(z.string())
        .parse(JSON.parse(readFileSync(join(dir, "args.json"), "utf8"))),
  };
}
afterEach(() => {
  for (const dir of directories.splice(0))
    rmSync(dir, { recursive: true, force: true });
});
const task = {
  id: "local-id",
  graph_id: "graph-id",
  title: "Call mum",
  status: "notStarted",
};
const ready = {
  schema_version: 2,
  sync: { state: "ready", generation: 4 },
  items: [task],
};

test("search reads and validates cached tasks with argv, including special input", async () => {
  const cli = fakeCli(JSON.stringify(ready));
  const result = await searchTasks("milk; $(touch /tmp/bad)", cli.path);
  assert.equal(result.items[0]?.id, "local-id");
  assert.deepEqual(cli.args(), [
    "--format",
    "json",
    "search",
    "milk; $(touch /tmp/bad)",
    "--status",
    "open",
  ]);
});
test("My Day preserves initial sync state", async () => {
  const cli = fakeCli(
    JSON.stringify({
      ...ready,
      sync: { state: "initial", generation: 0 },
      items: [],
    }),
  );
  assert.equal((await myDay(cli.path)).sync.state, "initial");
  assert.deepEqual(cli.args(), ["--format", "json", "myday", "list"]);
});
test("add and complete use one argv value and local ID", async () => {
  const cli = fakeCli(
    JSON.stringify({
      schema_version: 2,
      action: "add",
      op_id: "op",
      items: [task],
    }),
  );
  await addTask("Call mum #Home tomorrow", cli.path);
  assert.deepEqual(cli.args(), [
    "--format",
    "json",
    "tasks",
    "add",
    "Call mum #Home tomorrow",
    "--strict",
  ]);
  const done = fakeCli(
    JSON.stringify({
      schema_version: 2,
      action: "complete",
      op_id: "op",
      items: [task],
    }),
  );
  await completeTask(task.id, done.path);
  assert.deepEqual(done.args(), [
    "--format",
    "json",
    "tasks",
    "complete",
    "local-id",
  ]);
});
test("missing CLI gives installation guidance", () => {
  assert.throws(() => findCli("/missing/ms-todo"), CliError);
});
test("nonzero JSON stderr exposes CLI message", async () => {
  const cli = fakeCli(
    JSON.stringify({
      error: { kind: "not_signed_in", message: "sign in first" },
    }),
    2,
  );
  await assert.rejects(myDay(cli.path), /sign in first/);
});
test("invalid JSON, wrong schema, and malformed collections fail visibly", async () => {
  await assert.rejects(myDay(fakeCli("not json").path), /invalid JSON/);
  await assert.rejects(
    myDay(fakeCli(JSON.stringify({ ...ready, schema_version: 3 })).path),
    /unsupported JSON schema/,
  );
  await assert.rejects(
    myDay(
      fakeCli(
        JSON.stringify({
          ...ready,
          items: [{ title: "no id", status: "notStarted" }],
        }),
      ).path,
    ),
    /incomplete task collection/,
  );
});
test("mutation requires a confirmed action", async () => {
  await assert.rejects(
    addTask(
      "hello",
      fakeCli(JSON.stringify({ schema_version: 2, action: "add", items: [] }))
        .path,
    ),
    /did not confirm/,
  );
});
