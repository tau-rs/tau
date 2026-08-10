import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The whole point of this demo is to prove jco's transpile output loads under
// Vite. jco (with `--instantiation async`) emits `component.js` that pulls its
// core wasm via `new URL('./component.coreN.wasm', import.meta.url)` and
// instantiates it — a pattern Vite understands natively (it rewrites the
// import.meta.url asset URL at build). Two things make it work reliably:
//   - exclude @tau/embed-js / @tau/react from dep pre-bundling so the dynamic
//     `import('./generated/component.js')` resolves against the on-disk file
//     rather than a stale optimized copy;
//   - treat .wasm as an asset (jco loads it as a URL, not a JS module).
export default defineConfig({
  plugins: [react()],
  optimizeDeps: { exclude: ["@tau/embed-js", "@tau/react"] },
  assetsInclude: ["**/*.wasm"],
  // @tau/embed-js's loadTauInWorker uses `new Worker(new URL('./worker.ts',
  // import.meta.url), { type: 'module' })`. Vite statically bundles that
  // worker even though this demo only uses the main-thread loadTau; the worker
  // dynamically imports the jco output (code-splitting), which Vite's default
  // `iife` worker format rejects. ES-module workers allow the split.
  worker: { format: "es" },
});
