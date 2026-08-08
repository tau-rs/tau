import { agent, models, tool } from "tau";

export const m = models({ haiku: { backend: "anthropic", model: "claude-haiku-4-5" } });

const read_temp = tool({
    native: "ReadTemp",
    description: "Read the temperature sensor",
    capabilities: [{ kind: "fs.read", paths: ["/sys/**"] }],
});

export const monitor = agent({
    display_name: "Monitor",
    package: "monitor@^0.1",
    model: "haiku",
    prompt: { system: "Watch the temperature; report if above 30C." },
    tools: { read_temp },
});
