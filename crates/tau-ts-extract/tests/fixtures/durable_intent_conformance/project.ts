import { agent, models } from "tau";

export const projectModels = models({
  "mock-1": { backend: "mock-llm", model: "mock-1" },
});

export const fan = agent({
  display_name: "Fan",
  package: "p@^0.1",
  model: "mock-1",
  durable: "survive-restarts",
});
