export type FactCard = {
  label: string;
  title: string;
  body: string;
};

export type MatrixRow = {
  name: string;
  detail: string;
  status?: string;
};

export const productPosition = {
  eyebrow: "cost-optimized Rust coding agent",
  title: "Spend model tokens where they matter.",
  lead:
    "Teams burn expensive model cycles on repository discovery that local static analysis can do faster and more deterministically. Squeezy builds a semantic graph first, then feeds the agent compact code evidence instead of broad raw-file dumps."
};

export const homepageCards: FactCard[] = [
  {
    label: "CPU first",
    title: "Let local analysis do the repetitive work",
    body:
      "Squeezy maps the repository locally, then answers common navigation questions with compact evidence before the model spends context on source text."
  },
  {
    label: "Token budget",
    title: "Less context sent without losing the trail",
    body:
      "Graph tools return paths, spans, hashes, confidence labels, provenance, and next actions. Raw reads stay available, but they are narrowed to the exact slice when structure is enough."
  },
  {
    label: "Native runtime",
    title: "Fast local agent loop",
    body:
      "Squeezy runs as a native terminal agent with deterministic local work, explicit verification, and bounded tool output."
  }
];

export const optimizationCards: FactCard[] = [
  {
    label: "static graph",
    title: "Semantic navigation before file reads",
    body:
      "repo_map, declaration search, references, hierarchy, symbol context, upstream/downstream flow, and read_slice all work from local graph state and return compact evidence packets."
  },
  {
    label: "read shaping",
    title: "Exact slices, diff reads, and receipts",
    body:
      "Read tools can return bounded slices, changed ranges, receipt stubs for unchanged content, and spill handles for large output. The model gets enough evidence to act without paying for repeated bytes."
  },
  {
    label: "tool budget",
    title: "Budget counters visible in the session",
    body:
      "Per-turn counters track tool calls, read bytes, search hits, receipt hits, spills, denials, provider tokens, cache usage, and estimated cost when a provider exposes enough data."
  }
];

export const operatingLoop: FactCard[] = [
  {
    label: "1",
    title: "Index local code",
    body:
      "Squeezy parses supported files, discovers workspace facts, stores graph/cache partitions, and refreshes graph state as the workspace changes."
  },
  {
    label: "2",
    title: "Compile a focused evidence plan",
    body:
      "Common navigation prompts are routed through graph-first plans so the model starts with declarations, references, callers, hierarchy, and exact next actions."
  },
  {
    label: "3",
    title: "Escalate only when needed",
    body:
      "If graph evidence is incomplete, Squeezy falls back to bounded grep, glob, read_file, web, shell, or compiler tools behind the configured permission policy."
  },
  {
    label: "4",
    title: "Verify with local tools",
    body:
      "Builds, tests, formatters, linters, and benchmark commands provide compiler-backed evidence when the task needs it."
  }
];

export const toolSurface: FactCard[] = [
  {
    label: "navigation",
    title: "Graph-backed code tools",
    body:
      "Architecture maps, declarations, definitions, references, call candidates, hierarchy, symbol context, dependency flow, diff context, and exact read slices."
  },
  {
    label: "mutation",
    title: "Plan, patch, verify",
    body:
      "Plan mode hides mutation. Build mode exposes edit, shell, compiler, and git-style actions through capability checks, output shaping, and optional checkpoints."
  },
  {
    label: "support",
    title: "Local help, sessions, reports",
    body:
      "Squeezy can answer questions about itself from bundled docs before provider work, resume sessions, export/replay session logs, and prepare redacted feedback or report bundles."
  }
];

export const languageRows: MatrixRow[] = [
  {
    name: "Rust",
    detail: "Graph-backed navigation for modules, declarations, imports, references, calls, tests, and crate structure.",
    status: "premium graph support"
  },
  {
    name: "Python",
    detail: "Graph-backed navigation for classes, functions, imports, calls, decorators, bases, annotations, exports, and references.",
    status: "premium graph support"
  },
  {
    name: "Java",
    detail: "Graph-backed navigation for packages, imports, types, members, inheritance, calls, references, and project structure.",
    status: "premium graph support"
  },
  {
    name: "C#",
    detail: "Graph-backed navigation for namespaces, using directives, types, members, partial links, inheritance, references, and C# project files.",
    status: "premium graph support"
  },
  {
    name: "Go",
    detail: "Graph-backed navigation for packages, imports, structs, interfaces, type aliases, functions, methods, receivers, tests, calls, and references.",
    status: "premium graph support"
  },
  {
    name: "C",
    detail: "Graph-backed navigation for includes, structs, unions, enums, typedefs, fields, functions, macros, and references.",
    status: "premium graph support"
  },
  {
    name: "C++",
    detail: "Graph-backed navigation for includes, namespaces, classes, structs, methods, constructors, destructors, templates, operators, and references.",
    status: "premium graph support"
  },
  {
    name: "JavaScript",
    detail: "Graph-backed navigation for imports, exports, CommonJS aliases, functions, classes, object/member references, calls, and JSX declarations.",
    status: "premium graph support"
  },
  {
    name: "TypeScript",
    detail: "Graph-backed navigation for imports, exports, classes, interfaces, type aliases, enums, decorators, type references, calls, and TSX declarations.",
    status: "premium graph support"
  }
];

export const providerRows: MatrixRow[] = [
  {
    name: "OpenAI",
    detail: "Use OpenAI models while keeping repository indexing, permissions, tool shaping, and cost accounting in Squeezy."
  },
  {
    name: "Anthropic",
    detail: "Use Anthropic models with the same local graph navigation, tool loop, and session accounting."
  },
  {
    name: "Google Gemini",
    detail: "Use Gemini models without changing how Squeezy plans, narrows evidence, and verifies local work."
  },
  {
    name: "Azure OpenAI",
    detail: "Use Azure-hosted OpenAI models through your configured Azure endpoint and credentials."
  },
  {
    name: "Amazon Bedrock",
    detail: "Use models hosted through Amazon Bedrock with your AWS credentials and region configuration."
  },
  {
    name: "Ollama",
    detail: "Use a local Ollama model for local-first development when the selected model can follow the tool loop."
  }
];

export const installRows: MatrixRow[] = [
  {
    name: "macOS",
    detail: "Release targets: aarch64-apple-darwin for Apple Silicon and x86_64-apple-darwin for Intel. Primary install path: one-line curl installer.",
    status: "curl installer"
  },
  {
    name: "Linux",
    detail: "x86_64-unknown-linux-musl static binary. Primary install path: one-line curl installer.",
    status: "curl installer"
  },
  {
    name: "Windows",
    detail: "x86_64-pc-windows-msvc archive. Primary install path: Winget.",
    status: "winget"
  },
  {
    name: "Source build",
    detail: "Cargo install is available when you already have the required Rust toolchain.",
    status: "cargo"
  }
];

export const benchmarkFacts: FactCard[] = [
  {
    label: "scope",
    title: "Cost-saving benchmark page is under construction",
    body:
      "The public page will report how much context Squeezy avoids by using graph navigation, exact reads, receipts, and output shaping before model turns."
  },
  {
    label: "method",
    title: "Benchmarks need quality and cost together",
    body:
      "The benchmark method will pair navigation quality with measured read bytes, tool calls, receipt hits, spills, provider tokens, and baseline discovery effort."
  },
  {
    label: "status",
    title: "No public savings number yet",
    body:
      "Until the report is ready, the page should explain the measurement plan without publishing unsupported percentages or cost claims."
  }
];
