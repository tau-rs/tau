import { agent } from "tau";

export const judge = agent({
  display_name: "Judge",
  package: "p@^0.1",
  llm_backend: "mock-llm",
  model: "mock-1",
  outputSchema: { type: "object", required: ["verdict"] },
});
