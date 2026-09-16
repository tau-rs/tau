# Provider model capability table

Rendered from `drivers/tests/cassettes/*/probe_*.json` by `probes::render_models_md`; do not edit by hand.

A cell is `ok` when the recorded response was 2xx, `<status> <the start of the provider's message>` when it was not, and `—` where no cassette exists. OpenAI's gpt-5 and o-series reject the default `max_tokens` field with a 400 asking for `max_completion_tokens`, so on those models `text`, `tool call`, and `sampling present` all read 400 while `max_completion_tokens` reads `ok`: the same default cap field is sent on all three probes, so the request fails before the temperature question is ever reached, and that is the policy this table exists to record, not a driver bug. An id that answers 404 to `text` is not served by this endpoint at all (OpenAI's `pro` tier lives on `v1/responses`), so its other probes are skipped and read `—`. A snapshot (`-YYYY-MM-DD`, `-MMDD`) or context-window variant (`-16k`) of an alias the provider also lists is not probed: one model, one row. A model the provider retires keeps its cassettes and its row here, and is named under `retired` below, until someone deletes the files by hand.

## anthropic

| model | text | tool call | sampling present | thinking disabled |
|---|---|---|---|---|
| claude-fable-5 | ok | ok | 400 `temperature` is deprecated for this mod | 400 "thinking.type.disabled" is not supporte |
| claude-fable-5-1 | ok | ok | 400 `temperature` is deprecated for this mod | 400 "thinking.type.disabled" is not supporte |
| claude-haiku-4-5-20251001 | ok | ok | ok | ok |
| claude-opus-4-5-20251101 | ok | ok | ok | ok |
| claude-opus-4-6 | ok | ok | ok | ok |
| claude-opus-4-7 | ok | ok | 400 `temperature` is deprecated for this mod | ok |
| claude-opus-4-8 | ok | ok | 400 `temperature` is deprecated for this mod | ok |
| claude-opus-5 | ok | ok | 400 `temperature` is deprecated for this mod | ok |
| claude-sonnet-4-5-20250929 | ok | ok | ok | ok |
| claude-sonnet-4-6 | ok | ok | ok | ok |
| claude-sonnet-5 | ok | ok | 400 `temperature` is deprecated for this mod | ok |

## openai

| model | text | tool call | sampling present | max_completion_tokens |
|---|---|---|---|---|
| gpt-3.5-turbo | ok | ok | ok | ok |
| gpt-4 | ok | ok | ok | ok |
| gpt-4-turbo | ok | ok | ok | ok |
| gpt-4.1 | ok | ok | ok | ok |
| gpt-4.1-mini | ok | ok | ok | ok |
| gpt-4.1-nano | ok | ok | ok | ok |
| gpt-4o | ok | ok | ok | ok |
| gpt-4o-mini | ok | ok | ok | ok |
| gpt-5 | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5-mini | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5-nano | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5-pro | 404 This model is only supported in v1/respo | — | — | — |
| gpt-5.1 | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.2 | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.2-pro | 404 This is not a chat model and thus not su | — | — | — |
| gpt-5.4 | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.4-mini | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.4-nano | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.4-pro | 404 This is not a chat model and thus not su | — | — | — |
| gpt-5.5 | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.5-pro | 404 This is not a chat model and thus not su | — | — | — |
| gpt-5.6-luna | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.6-sol | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-5.6-terra | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| gpt-6-astra | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| o1 | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| o1-pro | 404 This model is only supported in v1/respo | — | — | — |
| o3 | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| o3-mini | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |
| o4-mini | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | 400 Unsupported parameter: 'max_tokens' is n | ok |

## inventory (at last record)

- retired (cassette, no longer listed): none
- unprobed (listed, no cassette): none
