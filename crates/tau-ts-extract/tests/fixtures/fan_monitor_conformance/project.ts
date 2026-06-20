export const projectModels = models({
    haiku: { backend: "anthropic", model: "claude-haiku-4-5" },
});

const read_temp = tool({
    native: "ReadTemp",
    description: "Read the temperature sensor",
});

const set_fan = tool({
    native: "SetFan",
    description: "Toggle the fan",
});

export const fan_monitor = agent({
    display_name: "Fan Monitor",
    package: "fan-monitor@^0.1",
    model: "haiku",
    prompt: { system: "Watch the temperature; turn on the fan if above 30°C." },
    tools: { read_temp, set_fan },
});

export const fan_pipeline = pipeline([
    { id: "check", run: "agent:fan_monitor" },
    { id: "act", run: "agent:fan_monitor", input: "${steps.check.output}" },
]);
