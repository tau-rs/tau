export const projectModels = models({
    haiku: { backend: "anthropic", model: "claude-haiku-4-5" },
});

const write_file = tool({
    native: "WriteFile",
    capabilities: [{ kind: "fs.write", paths: ["/workspace/**"] }],
});

export const gather = agent({
    display_name: "Gather",
    package: "research@^0.1",
    model: "haiku",
});

export const writer = agent({
    display_name: "Writer",
    package: "research@^0.1",
    model: "haiku",
    produces: ["/workspace/report.md"],
    tools: { write_file },
});

export const research_pipeline = pipeline([
    { id: "gather", run: "agent:gather", input: "${input}" },
    { id: "writer", run: "agent:writer", input: "${steps.gather.output}" },
]);

export const research_goals = goals([
    {
        id: "has_sources",
        evaluates: "/workspace/report.md",
        check: "matches",
        pattern: "(?m)^## Sources",
    },
]);

export const research_deliverables = deliverables([
    {
        id: "report",
        path: "/workspace/report.md",
        mustSatisfy: "A coherent summary.",
        onFail: "retry",
        maxAttempts: 3,
        retryFrom: "writer",
    },
]);
