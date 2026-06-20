import { agent, models } from "tau";

export const projectModels = models({
  "mock-1": { backend: "mock-llm", model: "mock-1" },
});

export const judge = agent({
  display_name: "Judge",
  package: "p@^0.1",
  model: "mock-1",
  outputSchema: { type: "object", required: ["verdict"] },
});
