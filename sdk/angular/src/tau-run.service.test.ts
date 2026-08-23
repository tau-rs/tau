import { lastValueFrom, toArray } from "rxjs";
import { describe, expect, it } from "vitest";
import type { RunEvent, TauComponent } from "@tau/embed-js";
import { TauRunService } from "./tau-run.service";

// A fake TauComponent that yields scripted events on microtask boundaries —
// mirroring a real streaming run without wasm or jco.
function fakeTau(events: RunEvent[]): TauComponent {
  return {
    run() {
      return (async function* () {
        for (const e of events) {
          await Promise.resolve();
          yield e;
        }
      })();
    },
  };
}

const completed: RunEvent = {
  type: "run-completed",
  outcome: {
    kind: "completed",
    finalMessage: "Hello",
    allMessages: ["Hello"],
    totalTurns: 1,
    tokenUsage: { inputTokens: 1, outputTokens: 2 },
  },
};

describe("TauRunService", () => {
  const svc = new TauRunService();

  it("run() emits every event in order and completes on stream close", async () => {
    const events: RunEvent[] = [
      { type: "run-started" },
      { type: "text-delta", delta: "Hi" },
      completed,
    ];
    const seen = await lastValueFrom(svc.run(fakeTau(events), { prompt: "hi" }).pipe(toArray()));
    expect(seen).toEqual(events);
  });

  it("text() scans text-delta into cumulative strings", async () => {
    const events: RunEvent[] = [
      { type: "run-started" },
      { type: "text-delta", delta: "Hel" },
      { type: "text-delta", delta: "lo" },
      completed,
    ];
    const emissions = await lastValueFrom(svc.text(fakeTau(events), { prompt: "hi" }).pipe(toArray()));
    expect(emissions).toEqual(["Hel", "Hello"]);
  });

  it("text() completes with no emissions when there are no text-deltas", async () => {
    const events: RunEvent[] = [{ type: "run-started" }, completed];
    const emissions = await lastValueFrom(svc.text(fakeTau(events), { prompt: "hi" }).pipe(toArray()));
    expect(emissions).toEqual([]);
  });

  it("run() delivers fatal-error as an ordinary terminal event, then completes", async () => {
    const fatal: RunEvent = { type: "fatal-error", kind: "Boom", detail: "kaboom" };
    const events: RunEvent[] = [{ type: "run-started" }, fatal];
    const seen = await lastValueFrom(svc.run(fakeTau(events), { prompt: "hi" }).pipe(toArray()));
    expect(seen[seen.length - 1]).toEqual(fatal);
  });
});
