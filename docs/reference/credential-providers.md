# Credential providers

The β.5 credential chain resolves a logical credential id through an
ordered list of providers. First match wins; a provider that does not
hold the credential is skipped; a configured provider that fails aborts
resolution (fail-fast).

## Providers shipped today

| Provider | `type` | Resolves from | Config |
|---|---|---|---|
| Env | `env` | process environment (`env` name from the declaration) | none (default) |
| File | `file` | `<dir>/<key_map[id]>` | `dir`, `key_map` |
| Baked | — | in-memory (tests/embedded) | constructed in code |

## Chain config (`[credentials]`, scope/home `config.toml`)

```toml
[credentials]
chain = ["env", "file"]

[credentials.providers.file]
type = "file"
dir  = "/var/run/secrets"
key_map = { anthropic_api_key = "anthropic-key" }
```

- `chain` is an ordered list of provider names. Empty or absent ⇒
  `["env"]`.
- `env` needs no `[credentials.providers.env]` entry.

## Per-agent declaration (`tau.toml`)

```toml
[[agents.<id>.credentials]]
id  = "anthropic_api_key"   # [a-z0-9_.-]
env = "ANTHROPIC_API_KEY"   # [A-Z_][A-Z0-9_]*
```

Validated at build time: bad `id`, bad `env`, or a duplicate `env`
within one agent is a build error.

## Deferred providers

`SecretManager` (Vault/AWS/GCP/Azure), `WorkloadIdentity` (SPIFFE/IRSA),
`DeviceIdentity` (secure-element), and `TokenBroker` (OIDC/OAuth2) are
reserved. The async port, `Ok(None)`/`Err` walk, byte `Secret`, and
`expires_at` rotation hook make each a non-breaking addition.
