# Build your first skill

By the end of this tutorial you will have written, installed, and
invoked a skill named `praise-poet` that responds to drafts with
upbeat affirmations. The point isn't poetry — the point is to see
each piece of tau's skill system work end-to-end on a single example
you wrote yourself.

You'll need: a working tau build (`cargo build --release` inside the
tau repo) and a text editor.

## What a skill is

A tau skill is a directory with two files:

- **`SKILL.md`** — the system prompt for an agent. Markdown body
  prefixed by YAML frontmatter (the same format Anthropic's Agent
  Skills uses).
- **`tau.toml`** — the package manifest. Names the skill, gives it a
  version, declares any capabilities it needs.

Anything else in the directory (reference files, examples, assets)
is part of the skill's payload: it travels with the skill on install
and is accessible at runtime.

We'll build all three files in `~/skills/praise-poet/`.

## Step 1 — Write SKILL.md

In a fresh directory, save the following as `SKILL.md`:

    ---
    name: praise-poet
    description: Responds to drafts with upbeat affirmations.
    ---

    You are an enthusiastic editor. When the user shares a draft,
    respond with three affirmations — one per paragraph. Each
    affirmation should quote a real phrase from the draft and call
    out what works about it.

    Keep it brief and specific. No filler superlatives ("amazing",
    "fantastic"). Quote the draft to anchor your praise.

The frontmatter is YAML. Both `name` and `description` are required.
Everything after the closing `---` is the prompt body — it becomes
the spawned agent's system prompt when this skill is invoked.

You could stop here and you'd have a valid Anthropic Agent Skill.
tau will pick it up too, but we'll add the package manifest next so
you can install it.

## Step 2 — Add the manifest

Save the following as `tau.toml` in the same directory:

    name = "praise-poet"
    version = "0.1.0"
    description = "Responds to drafts with upbeat affirmations."
    authors = ["you"]
    source = "local://praise-poet"
    kind = "skill"
    dependencies = []
    capabilities = []

    [skill]

The `name` here MUST match the `name` in your SKILL.md frontmatter.
tau enforces this on install (it's a guardrail against subtle
name-drift between the two files).

`capabilities = []` is correct for now — `praise-poet` doesn't need
to read files or run shell commands; it only needs the LLM.

## Step 3 — Install it

From your tau checkout:

    $ ./target/release/tau install ~/skills/praise-poet
    > Installed praise-poet@0.1.0

Verify with `tau skill list`:

    $ ./target/release/tau skill list
    Name           Version  Source
    ─────────────────────────────────────────
    praise-poet    0.1.0    local://praise-poet

And inspect the body:

    $ ./target/release/tau skill show praise-poet --body --raw

You should see your SKILL.md prompt printed back. `--raw` skips
markdown rendering; drop it to see the styled output.

## Step 4 — Invoke from an agent

This step depends on the agent surrounding your skill. The pattern
in tau is:

    [allow.models.default]
    backend = "anthropic"
    model   = "claude-haiku-4-5"

    [agents.reviewer]
    package = "code-reviewer@^0.1"
    model = "default"   # the alias — not "claude-haiku-4-5"

    [[agents.reviewer.capabilities]]
    kind = "skill.spawn"
    allowed_skills = ["praise-poet"]

Now the `reviewer` agent can emit `skill.praise-poet.spawn` as a
tool call, and tau will spawn a child agent backed by your skill.

See [How-to: install a skill](../how-to/install-a-skill.md) and
[How-to: author a skill](../how-to/author-a-skill.md) for more on
the capability-declaration patterns. See
[Reference: skill manifest schema](../reference/skill-manifest-schema.md)
for the complete schema.

## Step 5 — Export back to Anthropic

If you want to share your skill with the broader Anthropic
ecosystem (claude-code, for example):

    $ ./target/release/tau skill export praise-poet --output ./out
    > Exported praise-poet to ./out

The `./out/` directory now contains the SKILL.md — no tau.toml,
since the Anthropic format doesn't carry that. You can hand it to
anyone using the Anthropic skill format and they'll be able to use
your prompt.

For skills that declare capabilities, `tau skill export` drops them
with a warning (Anthropic format doesn't preserve capabilities). If
your skill had declared, say, `fs.read`, the export would emit:

    $ ./target/release/tau skill export my-skill --output ./out
    note: 1 capabilities dropped on Anthropic export (fs.read);
          Anthropic format does not preserve capability declarations

## What's next

- **Bundle reference files** with your skill — the
  [how-to on authoring](../how-to/author-a-skill.md) shows the
  `${SKILL_DIR}` substitution pattern for reading bundled files at
  invocation time.
- **Declare capabilities** — fs.read, process.spawn, etc. The
  [reference page](../reference/skill-manifest-schema.md) covers the
  complete set.
- **Read the design** — [explanation: two-layer skills](../explanation/two-layer-skills.md)
  walks through why tau picked this architecture and what trade-offs
  it locked in.
