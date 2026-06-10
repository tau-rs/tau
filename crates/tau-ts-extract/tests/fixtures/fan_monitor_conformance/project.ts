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
    llm_backend: "anthropic",
    model: "claude-haiku-4-5",
    prompt: { system: "Watch the temperature; turn on the fan if above 30°C." },
    tools: { read_temp, set_fan },
});
