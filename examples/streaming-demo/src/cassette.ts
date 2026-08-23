// A synchronous cassette `complete` host import.
//
// `tau:host/host`'s `complete` is a *sync* WIT import and @tau/embed-js
// transpiles the component in jco's default sync mode, so this must return a
// serialized `CompletionResponse` string without awaiting. That is exactly
// the intended use of the sync path: preloaded / cassette responses, not live
// network inference (which needs a `jco --async-mode jspi` build).
//
// The guest's one agent calls `complete` once with a serialized
// `CompletionRequest`; we ignore it and return a canned reply whose `text`
// the runtime streams back as the assistant message.
export function cassetteComplete(_requestJson: string): string {
  return JSON.stringify({
    text: "Hello from a tau wasm component — streamed live into React via @tau/embed-js and useTauRun.",
    tool_uses: [],
    stop_reason: "EndTurn",
    usage: null,
  });
}
