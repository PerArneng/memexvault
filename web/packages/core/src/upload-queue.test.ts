import { vaultId } from "@memex/types";
import type { DocumentSource } from "@memex/types";
import { describe, expect, it } from "vitest";
import { formatBytes, isRecordingFile, MAX_CONCURRENT_UPLOADS, createUploadQueue, uploadRefusal } from "./upload-queue";
import { failed, ok } from "./api";
import { scriptedUploader } from "./ingest-testing";
import { httpFailure, UNREACHABLE } from "./testing";

const limits = { maxBytes: 1000, allowedMediaTypes: ["application/pdf", "text/plain", "audio/mp4", "audio/mpeg"] };
const file = (name: string, size = 10, type = "application/pdf") => ({ name, size, type });
const source = (id: string) => ({ id, name: `${id}.pdf` }) as DocumentSource;
const options = { trustTier: "unverified", language: null };

const setup = () => {
  const uploader = scriptedUploader();
  let n = 0;
  const queue = createUploadQueue({ vault: vaultId("v"), limits, uploader, nextId: () => `id-${String((n += 1))}` });
  const states = () => queue.state().items.map((i) => [i.name, i.state.kind]);
  return { uploader, queue, states };
};

describe("uploadRefusal", () => {
  it("refuses what's over the published limit, and says both sizes", () => {
    expect(uploadRefusal(file("big.pdf", 2000), limits)).toBe("This is 2 KB, and the most this server takes is 1000 bytes.");
    expect(uploadRefusal(file("fits.pdf", 1000), limits)).toBeNull();
  });
  it("refuses a type it's sure of, and an empty file", () => {
    expect(uploadRefusal(file("a.zip", 10, "application/zip"), limits)).toContain("doesn't take application/zip");
    expect(uploadRefusal(file("a.pdf", 0), limits)).toBe("This file is empty.");
  });
  it("leaves to the server what only its first bytes can decide", () => {
    expect(uploadRefusal(file("mystery", 10, ""), limits)).toBeNull();
    expect(uploadRefusal(file("a.bin", 10, "application/octet-stream"), limits)).toBeNull();
    expect(uploadRefusal(file("data.csv", 10, "text/csv"), limits)).toBeNull();
  });
  it("trusts a known extension over a browser's odd type", () => {
    expect(uploadRefusal(file("talk.m4a", 10, "audio/x-m4a"), limits)).toBeNull();
    expect(isRecordingFile(file("talk.m4a", 10, ""))).toBe(true);
    expect(isRecordingFile(file("a.pdf"))).toBe(false);
  });
  it("formats sizes", () => {
    expect(formatBytes(2 * 1024 ** 3)).toBe("2.0 GB");
    expect(formatBytes(1536 * 1024)).toBe("1.5 MB");
  });
});

describe("createUploadQueue", () => {
  it("never sends what's refused", () => {
    const { uploader, queue, states } = setup();
    queue.add([file("big.pdf", 5000), file("a.zip", 10, "application/zip")], options);
    expect(states()).toEqual([["big.pdf", "refused"], ["a.zip", "refused"]]);
    expect(uploader.started).toEqual([]);
  });

  it("runs a limited number at once and starts the next when one ends", async () => {
    const { uploader, queue, states } = setup();
    queue.add([file("1.pdf"), file("2.pdf"), file("3.pdf")], options);
    expect(uploader.started.length).toBe(MAX_CONCURRENT_UPLOADS);
    expect(states()[2]).toEqual(["3.pdf", "queued"]);
    await uploader.finish("1.pdf", ok({ source: source("s1"), existing: false, jobId: "j" }));
    expect(states()[0]).toEqual(["1.pdf", "stored"]);
    expect(uploader.started.map((s) => s.name)).toContain("3.pdf");
  });

  it("reports progress per file", () => {
    const { uploader, queue } = setup();
    queue.add([file("1.pdf")], options);
    uploader.progress("1.pdf", 7);
    expect(queue.state().items[0]?.state).toEqual({ kind: "uploading", sent: 7 });
  });

  it("names a duplicate as such", async () => {
    const { uploader, queue, states } = setup();
    queue.add([file("1.pdf")], options);
    await uploader.finish("1.pdf", ok({ source: source("older"), existing: true, jobId: null }));
    expect(states()).toEqual([["1.pdf", "duplicate"]]);
    expect(queue.state().items[0]?.state).toMatchObject({ source: { id: "older" } });
  });

  it("explains a lost connection, and a retry sends the same file again", async () => {
    const { uploader, queue, states } = setup();
    queue.add([file("1.pdf")], { trustTier: "trusted", language: "sv" });
    await uploader.finish("1.pdf", failed(UNREACHABLE));
    expect(queue.state().items[0]?.state).toMatchObject({ kind: "failed", reason: expect.stringContaining("connection was lost") as string });
    const id = queue.state().items[0]?.id ?? "";
    queue.retry(id);
    expect(states()).toEqual([["1.pdf", "uploading"]]);
    expect(uploader.started.at(-1)).toEqual({ name: "1.pdf", options: { trustTier: "trusted", language: "sv" } });
  });

  it("reports the server's refusals in words", async () => {
    const { uploader, queue } = setup();
    queue.add([file("a"), file("b")], options);
    await uploader.finish("a", failed(httpFailure(415, "unsupported_media_type")));
    await uploader.finish("b", failed({ ...httpFailure(429, "too_many_jobs"), retryAfterS: 9 } as never));
    const [a, b] = queue.state().items;
    expect(a?.state).toMatchObject({ kind: "failed", reason: expect.stringContaining("doesn't take its type") as string });
    expect(b?.state).toMatchObject({ kind: "failed", reason: expect.stringContaining("Try again in 9 s") as string });
  });

  it("cancel aborts, starts the next, and ignores how the request ended", async () => {
    const { uploader, queue, states } = setup();
    queue.add([file("1.pdf"), file("2.pdf"), file("3.pdf")], options);
    const id = queue.state().items[0]?.id ?? "";
    queue.cancel(id);
    expect(uploader.aborted).toEqual(["1.pdf"]);
    expect(states()).toEqual([["1.pdf", "cancelled"], ["2.pdf", "uploading"], ["3.pdf", "uploading"]]);
    await uploader.finish("1.pdf", failed(UNREACHABLE));
    expect(states()[0]).toEqual(["1.pdf", "cancelled"]);
  });

  it("remove forgets it, and stop aborts what's running", () => {
    const { uploader, queue } = setup();
    queue.add([file("1.pdf"), file("2.pdf")], options);
    queue.remove(queue.state().items[0]?.id ?? "");
    expect(queue.state().items.map((i) => i.name)).toEqual(["2.pdf"]);
    queue.stop();
    expect(uploader.aborted).toEqual(["1.pdf", "2.pdf"]);
  });
});
