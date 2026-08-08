from tau_sdk import agent, models, model, tool, print_toml

m = models(haiku=model(backend="anthropic", model="claude-haiku-4-5"))

read_temp = tool(
    native="ReadTemp",
    description="Read the temperature sensor",
    capabilities=[{"kind": "fs.read", "paths": ["/sys/**"]}],
)

monitor = agent(
    display_name="Monitor",
    package="monitor@^0.1",
    model="haiku",
    prompt={"system": "Watch the temperature; report if above 30C."},
    tool_refs=["read_temp"],
)

print_toml(
    project="tool-agent",
    models=m,
    agents={"monitor": monitor},
    tools={"read_temp": read_temp},
)
