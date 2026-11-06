// Execute the actual production save functions with deferred local IPC replies.
// No DOM, app data, recording, model, or network access is involved.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = ts.createSourceFile(
  "main.ts",
  readFileSync(new URL("../src/main.ts", import.meta.url), "utf8"),
  ts.ScriptTarget.Latest,
  true,
);
const names = new Set(["persist", "flushNotes"]);
const functions = source.statements.filter(
  (statement) =>
    ts.isFunctionDeclaration(statement) && names.has(statement.name?.text),
);
assert.equal(
  functions.length,
  names.size,
  "production save entry points exist",
);
const compiled = ts.transpileModule(
  functions.map((statement) => statement.getText(source)).join("\n"),
  { compilerOptions: { target: ts.ScriptTarget.ES2022 } },
).outputText;
const load = new Function(
  "timers",
  "queues",
  "drafts",
  "invoke",
  `${compiled}\nreturn { persist, flushNotes };`,
);

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function harness() {
  const first = deferred(),
    second = deferred();
  const replies = [first, second];
  const drafts = new Map(),
    queues = new Map(),
    calls = [],
    saved = new Map();
  const api = load(new Map(), queues, drafts, (command, args) => {
    assert.equal(command, "save_note");
    calls.push(args.content);
    const reply = replies[calls.length - 1]?.promise || Promise.resolve();
    return reply.then(() => {
      saved.set(args.noteId, args.content);
      return {};
    });
  });
  const id = "synthetic-note";
  function put(content) {
    drafts.set(id, {
      note: { id, meeting_id: "synthetic-meeting", anchor_ms: 0 },
      content,
      kind: "document",
    });
  }
  function start(content) {
    put(content);
    // Observe errors immediately, just as the UI's guard does. The underlying
    // queue remains rejected so subsequent saves exercise its real recovery.
    return api.persist(id).then(
      () => ({ ok: true }),
      (error) => ({ ok: false, error }),
    );
  }
  async function called(count) {
    for (let tick = 0; tick < 10 && calls.length < count; tick++) {
      await new Promise(setImmediate);
    }
    assert.equal(calls.length, count);
  }
  return {
    ...api,
    first,
    second,
    drafts,
    calls,
    saved,
    id,
    put,
    start,
    called,
  };
}

test("failed A cannot be restored after B is queued and saved", async () => {
  const h = harness();
  const a = h.start("A");
  await h.called(1);
  const b = h.start("B");
  assert.deepEqual(h.calls, ["A"], "B still waits for A's reply");
  h.first.reject(new Error("synthetic A failure"));
  assert.equal((await a).ok, false);
  await h.called(2);
  h.second.resolve();
  assert.equal((await b).ok, true);
  await h.flushNotes();
  assert.deepEqual(h.calls, ["A", "B"], "flush must not resend stale A");
  assert.equal(h.saved.get(h.id), "B");
  assert.equal(h.drafts.has(h.id), false);
});

test("when A and B both fail, retry restores and saves B", async () => {
  const h = harness();
  const a = h.start("A");
  await h.called(1);
  const b = h.start("B");
  h.first.reject(new Error("synthetic A failure"));
  assert.equal((await a).ok, false);
  await h.called(2);
  h.second.reject(new Error("synthetic B failure"));
  assert.equal((await b).ok, false);
  assert.equal(h.drafts.get(h.id)?.content, "B");
  await h.flushNotes();
  assert.deepEqual(h.calls, ["A", "B", "B"]);
  assert.equal(h.saved.get(h.id), "B");
});

test("pending failures cannot replace a newer unsent C draft", async () => {
  const h = harness();
  const a = h.start("A");
  await h.called(1);
  const b = h.start("B");
  h.put("C");
  h.first.reject(new Error("synthetic A failure"));
  assert.equal((await a).ok, false);
  await h.called(2);
  h.second.reject(new Error("synthetic B failure"));
  assert.equal((await b).ok, false);
  assert.equal(h.drafts.get(h.id)?.content, "C");
  await h.flushNotes();
  assert.deepEqual(h.calls, ["A", "B", "C"]);
  assert.equal(h.saved.get(h.id), "C");
});

test("the latest failed save remains available for retry", async () => {
  const h = harness();
  const a = h.start("A");
  await h.called(1);
  h.first.reject(new Error("synthetic A failure"));
  assert.equal((await a).ok, false);
  assert.equal(h.drafts.get(h.id)?.content, "A");
  const retry = h.flushNotes();
  await h.called(2);
  h.second.resolve();
  await retry;
  assert.deepEqual(h.calls, ["A", "A"]);
  assert.equal(h.saved.get(h.id), "A");
  assert.equal(h.drafts.has(h.id), false);
});
