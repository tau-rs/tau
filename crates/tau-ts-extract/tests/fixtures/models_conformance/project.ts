export const projectModels = models({
    haiku: { backend: "anthropic", model: "claude-haiku-4-5" },
    opus: { backend: "anthropic", model: "claude-opus-4-8" },
});

export const fast = agent({
    display_name: "Fast",
    package: "research@^0.1",
    model: "haiku",
});

export const deep = agent({
    display_name: "Deep",
    package: "research@^0.1",
    model: "opus",
});
