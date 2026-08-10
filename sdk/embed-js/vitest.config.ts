import { defineConfig } from "vitest/config";

// Runs the hand-authored TS unit tests (currently src/normalize.test.ts) in a
// plain Node environment. `normalize.ts` is pure JSON-shape mapping — no DOM,
// no wasm — so `node` is sufficient and keeps the suite dependency-free beyond
// vitest itself.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
