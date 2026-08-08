from tau_sdk import agent, models, model, print_toml

m = models(haiku=model(backend="anthropic", model="claude-haiku-4-5"))
fast = agent(display_name="Fast", package="research@^0.1", model="haiku")

print_toml(project="basic-agent", models=m, agents={"fast": fast})
