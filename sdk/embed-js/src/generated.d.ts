// Ambient type for jco's transpile output (src/generated/component.js) — a
// gitignored build artifact that does not exist until `npm run build`.
// Declaring it lets this package's .ts source (index.ts, worker.ts) and any
// downstream consumer of the .ts entry (@tau/react, @tau/angular) typecheck
// without the generated module present. index.ts/worker.ts cast the dynamic
// import to their own `GeneratedModule` shape, so the loose typing here is
// deliberate; when a real build is present, actual module resolution wins over
// this wildcard and the cast still holds.
declare module "*/generated/component.js" {
  export function instantiate(
    getCoreModule: undefined,
    imports: { "tau:host/host": unknown },
  ): unknown;
}
