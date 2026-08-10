import { useEffect, useState } from "react";
import { loadTau, type TauComponent } from "@tau/embed-js";
import { useTauRun } from "@tau/react";
import { cassetteComplete } from "./cassette";

export function App() {
  const [tau, setTau] = useState<TauComponent | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    // loadTau instantiates the jco-transpiled component (bundled by @tau/embed-js
    // into ./generated). The cassette bridges the sync `complete` host import.
    loadTau({ complete: cassetteComplete })
      .then((t) => live && setTau(t))
      .catch((e: unknown) => live && setLoadError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, []);

  if (loadError) return <p role="alert">Failed to load component: {loadError}</p>;
  if (!tau) return <p>Loading tau wasm component…</p>;
  return <Runner tau={tau} />;
}

function Runner({ tau }: { tau: TauComponent }) {
  const { start, text, status, events, error } = useTauRun(tau);
  return (
    <main style={{ fontFamily: "system-ui, sans-serif", maxWidth: 640, margin: "3rem auto" }}>
      <h1>tau streaming demo</h1>
      <p>
        A <code>tau build wasm</code> component, transpiled by jco, streaming its{" "}
        <code>RunEvent</code>s into React via <code>useTauRun</code>.
      </p>
      <button onClick={() => start({ prompt: "hello" })} disabled={status === "running"}>
        {status === "running" ? "Running…" : "Run"}
      </button>
      <p>
        status: <strong>{status}</strong> · events: {events.length}
      </p>
      <pre
        aria-label="streamed-text"
        style={{ background: "#f4f4f5", padding: "1rem", borderRadius: 8, whiteSpace: "pre-wrap" }}
      >
        {text || " "}
      </pre>
      {status === "error" && (
        <p role="alert">
          {error?.kind}: {error?.detail}
        </p>
      )}
    </main>
  );
}
