import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { RunEvent, TauComponent } from "@tau/embed-js";
import { useTauRun } from "./useTauRun";

// A fake TauComponent that yields scripted events on microtask boundaries, so
// each event is a distinct render — mirroring a real streaming run without
// wasm or jco.
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

describe("useTauRun", () => {
  it("starts idle with empty state", () => {
    const { result } = renderHook(() => useTauRun(fakeTau([])));
    expect(result.current.status).toBe("idle");
    expect(result.current.text).toBe("");
    expect(result.current.events).toEqual([]);
    expect(result.current.outcome).toBeNull();
    expect(result.current.error).toBeNull();
  });

  it("concatenates text-delta and settles done with outcome", async () => {
    const events: RunEvent[] = [
      { type: "run-started" },
      { type: "text-delta", delta: "Hel" },
      { type: "text-delta", delta: "lo" },
      completed,
    ];
    const { result } = renderHook(() => useTauRun(fakeTau(events)));
    act(() => result.current.start({ prompt: "hi" }));
    await waitFor(() => expect(result.current.status).toBe("done"));
    expect(result.current.text).toBe("Hello");
    expect(result.current.events).toHaveLength(4);
    expect(result.current.outcome).toEqual(completed.outcome);
    expect(result.current.error).toBeNull();
  });

  it("surfaces fatal-error as error status", async () => {
    const events: RunEvent[] = [
      { type: "run-started" },
      { type: "fatal-error", kind: "Boom", detail: "kaboom" },
    ];
    const { result } = renderHook(() => useTauRun(fakeTau(events)));
    act(() => result.current.start({ prompt: "hi" }));
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current.error).toEqual({ kind: "Boom", detail: "kaboom" });
    expect(result.current.outcome).toBeNull();
  });

  it("settles done on a clean end-of-stream without a terminal event", async () => {
    const events: RunEvent[] = [
      { type: "run-started" },
      { type: "text-delta", delta: "x" },
    ];
    const { result } = renderHook(() => useTauRun(fakeTau(events)));
    act(() => result.current.start({ prompt: "hi" }));
    await waitFor(() => expect(result.current.status).toBe("done"));
    expect(result.current.text).toBe("x");
    expect(result.current.outcome).toBeNull();
  });

  it("resets state when restarted", async () => {
    const first = fakeTau([{ type: "text-delta", delta: "one" }, completed]);
    const { result, rerender } = renderHook(({ tau }) => useTauRun(tau), {
      initialProps: { tau: first },
    });
    act(() => result.current.start({ prompt: "1" }));
    await waitFor(() => expect(result.current.status).toBe("done"));
    expect(result.current.text).toBe("one");

    const second = fakeTau([{ type: "text-delta", delta: "two" }]);
    rerender({ tau: second });
    act(() => result.current.start({ prompt: "2" }));
    // Reset is synchronous on start.
    expect(result.current.text).toBe("");
    expect(result.current.status).toBe("running");
    await waitFor(() => expect(result.current.status).toBe("done"));
    expect(result.current.text).toBe("two");
  });
});
