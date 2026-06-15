# Use a mounted secret as a credential

This guide shows how to feed a Kubernetes / Docker mounted secret to an
agent's plugin **without changing the plugin** — using the β.5 credential
chain.

## 1. Declare the credential on the agent (`tau.toml`)

```toml
[agents.assistant]
llm_backend = "anthropic"

[[agents.assistant.credentials]]
id  = "anthropic_api_key"
env = "ANTHROPIC_API_KEY"
```

`id` is the logical name the chain resolves; `env` is the variable the
plugin already reads. This declaration travels in the bundle.

## 2. Configure the chain for the deployment (scope/home `config.toml`)

```toml
[credentials]
chain = ["env", "file"]

[credentials.providers.file]
type = "file"
dir  = "/var/run/secrets"
key_map = { anthropic_api_key = "anthropic-key" }
```

The host tries `env` first (today's behavior), then reads
`/var/run/secrets/anthropic-key`.

## 3. Mount the secret

Mount your secret so the file lands at `/var/run/secrets/anthropic-key`.
The host resolves it and injects it as `ANTHROPIC_API_KEY` into the
plugin process. The unmodified plugin reads it exactly as before.

## Zero-config default

With no `[credentials]` block and no `credentials` declaration, the
behavior is identical to earlier tau: each plugin reads its own env var.
