import { describe, expect, it } from "vitest";
import { createSseParser } from "./sse-parser";
import type { SseMessage } from "./sse-parser";

const parse = (chunks: readonly string[]) => {
  const out: SseMessage[] = [];
  const parser = createSseParser((m) => out.push(m));
  for (const chunk of chunks) parser.feed(chunk);
  return out;
};

describe("createSseParser", () => {
  it("reads named frames with ids", () => {
    expect(parse(['id: 7\nevent: durable\ndata: {"seq":7}\n\n'])).toEqual([{ event: "durable", data: '{"seq":7}', id: "7" }]);
  });
  it("copes with chunks that end anywhere, including inside a line ending", () => {
    const text = "event: progress\r\ndata: 1\r\n\r\nevent: hello\ndata: 2\n\n";
    const whole = parse([text]);
    expect(whole.map((m) => m.event)).toEqual(["progress", "hello"]);
    expect(parse(text.split(""))).toEqual(whole);
  });
  it("ignores comments and joins data lines", () => {
    expect(parse([": keepalive\n\ndata: a\ndata: b\n\n"])).toEqual([{ event: "message", data: "a\nb", id: null }]);
  });
  it("holds back an unfinished frame", () => {
    expect(parse(["event: durable\ndata: 1\n"])).toEqual([]);
  });
});
