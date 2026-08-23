import { defineConfig } from "vitest/config";

// TauRunService carries an `@Injectable` (legacy TS) decorator, so the test
// transform must run in experimental-decorator mode. The service is pure
// RxJS over an async iterable — no DI, no zone.js, no DOM — so a plain node
// environment is enough; tests instantiate it with `new` and feed a fake
// TauComponent.
export default defineConfig({
  esbuild: {
    tsconfigRaw: {
      compilerOptions: {
        experimentalDecorators: true,
      },
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
