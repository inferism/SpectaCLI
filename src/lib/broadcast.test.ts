import { expect, test } from "vitest";
import { type BroadcastTarget, broadcast, pasteBytes } from "./broadcast";

test("agents get a bracketed paste, shells plain text", () => {
  expect(pasteBytes("hi\nthere", true)).toBe("\x1b[200~hi\nthere\x1b[201~");
  expect(pasteBytes("hi\r\nthere", false)).toBe("hi there");
});

test("escape characters cannot end the paste early", () => {
  expect(pasteBytes("a\x1b[201~rm -rf", true)).toBe("\x1b[200~a[201~rm -rf\x1b[201~");
});

test("every target gets the paste before any gets its submit key", async () => {
  const log: string[] = [];
  const targets: BroadcastTarget[] = [
    { ptyId: "a", submit: "\r", bracketed: true },
    { ptyId: "b", submit: "\n", bracketed: false },
  ];
  const failed = await broadcast(
    "go",
    targets,
    async (id, d) => void log.push(`${id}:${JSON.stringify(d)}`),
    async () => void log.push("wait"),
  );
  expect(failed).toEqual([]);
  expect(log).toEqual([`a:${JSON.stringify("\x1b[200~go\x1b[201~")}`, 'b:"go"', "wait", 'a:"\\r"', 'b:"\\n"']);
});

test("a target that fails is reported and not submitted", async () => {
  const sent: string[] = [];
  const failed = await broadcast(
    "x",
    [
      { ptyId: "dead", submit: "\r", bracketed: true },
      { ptyId: "ok", submit: "\r", bracketed: true },
    ],
    async (id, d) => {
      if (id === "dead") throw "gone";
      sent.push(d);
    },
    async () => {},
  );
  expect(failed).toEqual(["dead"]);
  expect(sent).toHaveLength(2);
});
