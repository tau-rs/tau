import { agent, models } from "tau";

export const m = models({ haiku: { backend: "anthropic", model: "claude-haiku-4-5" } });
export const fast = agent({ display_name: "Fast", package: "research@^0.1", model: "haiku" });
