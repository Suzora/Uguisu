import { afterEach, describe, expect, it, vi } from "vitest";
import { announce } from "./desktop-notifications";
import type { UguisuEvent } from "./api";
import { stubApi } from "../tests/harness";

afterEach(() => {
  vi.unstubAllGlobals();
});

/** Announces a failed download and returns the toast the shell was asked for. */
async function failed(attempts: number): Promise<unknown> {
  const invoke = vi.fn(() => Promise.resolve());
  vi.stubGlobal("__TAURI__", { core: { invoke } });
  stubApi({
    "GET /api/v1/downloads/01JOB": {
      body: { job: { episode_id: "01EP" }, schema: 1 },
    },
    "GET /api/v1/episodes/01EP": {
      body: {
        episode: { title: "Episode 7" },
        podcast_title: "The Show",
        schema: 1,
      },
    },
  });
  await announce({
    schema: 1,
    id: "01EV",
    occurred_at: "2026-09-26T12:00:00Z",
    podcast_id: null,
    episode_id: null,
    kind: "download.failed",
    job_id: "01JOB",
    reason: attempts > 1 ? "max_attempts" : "target_exists",
    attempts,
  } as unknown as UguisuEvent);
  return invoke.mock.calls.at(-1);
}

describe("desktop notifications", () => {
  it("counts the attempts given up after", async () => {
    expect(await failed(8)).toEqual([
      "notify",
      {
        title: "The Show: download failed",
        body: "Episode 7 — Uguisu gave up after 8 attempts.",
      },
    ]);
  });

  it("claims no retry after one attempt", async () => {
    expect(await failed(1)).toEqual([
      "notify",
      {
        title: "The Show: download failed",
        body: "Episode 7 — It was not retried.",
      },
    ]);
  });
});
