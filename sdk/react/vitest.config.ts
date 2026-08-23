import { defineConfig } from "vitest/config";

// The hook drives React state from an async RunEvent stream, so tests render
// it in a DOM. jsdom is sufficient — no real wasm or network is involved; the
// tests feed a fake TauComponent that yields scripted events.
export default defineConfig({
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
