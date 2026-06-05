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

export type BenchmarkRow = {
  lang: string;
  squeezyCost: number;
  baselineCost: number;
  ratio: number;
  recall: number;
  verdict: "WIN" | "LOSS";
};

export const productPosition = {
  eyebrow: "coding agent",
  title: "Spend model tokens on solving code, not rediscovering your repository.",
  lead:
    "Squeezy is a terminal coding agent built to keep LLM context focused, reduce repeated code discovery, and make provider spend visible.",
  note: "Built with speed in mind using Rust."
};

export const heroMetrics = [
  { label: "Codex comparison", value: "15 / 15 lower cost", detail: "same-task Mini benchmark" },
  { label: "Languages", value: "15", detail: "supported for local code understanding" },
  { label: "Cost levers", value: "11", detail: "implemented layers from the cost docs" },
  { label: "Providers", value: "BYO model", detail: "native, compatible, and local routes" }
];

export const costLeaks: FactCard[] = [
  {
    label: "repo discovery",
    title: "The model keeps finding the same code",
    body:
      "Broad searches, whole-file reads, and repeated navigation can turn a simple coding task into a large paid context."
  },
  {
    label: "tool output",
    title: "Commands return more than the model needs",
    body:
      "Build logs, test output, grep results, diffs, and images can flood the next request when they are sent raw."
  },
  {
    label: "long sessions",
    title: "Old context becomes permanent baggage",
    body:
      "Without compaction, every turn carries yesterday's exploration, stale tool output, and already-settled decisions."
  },
  {
    label: "model choice",
    title: "Every prompt uses the expensive path",
    body:
      "Small mechanical turns should not always pay for the same model path as hard design or debugging work."
  }
];

export const costPillars: FactCard[] = [
  {
    label: "prompt reuse",
    title: "Reuse stable prompt context",
    body:
      "When a provider supports prompt caching, Squeezy keeps stable instructions and tool context cache-friendly so repeated turns can be cheaper."
  },
  {
    label: "compaction",
    title: "Keep long sessions bounded",
    body:
      "Older conversation state is summarized into goal, progress, decisions, and next steps while recent work stays available."
  },
  {
    label: "receipts",
    title: "Avoid paying twice for the same output",
    body:
      "Repeated file reads or command results can be represented by a small receipt with a recovery path instead of resending the bytes."
  },
  {
    label: "shaped output",
    title: "Send the useful part of command output",
    body:
      "Squeezy trims noisy build, test, search, diff, and shell output into focused blocks that the model can act on."
  },
  {
    label: "targeted reads",
    title: "Read relevant code before broad context",
    body:
      "Local code understanding helps the agent choose the files and slices that matter before spending model context on source text."
  },
  {
    label: "lazy loading",
    title: "Load tools and skills only when needed",
    body:
      "The model sees a compact index first, then asks for full tool schemas or skill instructions only when a task needs them."
  },
  {
    label: "resume",
    title: "Resume without replaying everything",
    body:
      "Session state, checkpoints, and memory let longer projects continue from compact anchors instead of rebuilding the full past."
  },
  {
    label: "subagents",
    title: "Keep exploration off the main thread",
    body:
      "Short-lived subagents can research, review, or inspect docs and return summaries instead of expanding the parent conversation."
  },
  {
    label: "verbosity",
    title: "Control how much gets said and returned",
    body:
      "Response and tool-output verbosity settings keep normal turns concise, with full detail still available when needed."
  },
  {
    label: "accounting",
    title: "Show where tokens go",
    body:
      "Cost and context views separate input, output, cached input, tool bytes, reasoning, and estimates where providers expose the data."
  },
  {
    label: "routing",
    title: "Use cheaper routes for simple turns",
    body:
      "Obvious mechanical requests can start on a provider-local small model and escalate when the task becomes complex."
  }
];

export const productSubjects: FactCard[] = [
  {
    label: "coding first",
    title: "A terminal agent for real code work",
    body:
      "Squeezy can inspect code, edit files, run commands, manage plans, resume sessions, and keep model work tied to local evidence."
  },
  {
    label: "languages",
    title: "Language-aware code understanding",
    body:
      "Fifteen supported languages get local code understanding and navigation before the model reaches for broad file context."
  },
  {
    label: "permissions",
    title: "Reviewable local actions",
    body:
      "File edits, shell commands, web access, MCP calls, destructive actions, and outside-workspace paths stay behind configurable policies."
  },
  {
    label: "sessions",
    title: "Work can be resumed and audited",
    body:
      "Local logs, resume state, reports, labels, forks, feedback, and optional checkpoints keep long coding sessions inspectable."
  },
  {
    label: "providers",
    title: "Bring your preferred model",
    body:
      "Use native providers, compatible endpoints, OAuth-style routes, or local runtimes while Squeezy keeps the optimization local."
  },
  {
    label: "docs",
    title: "Technical detail stays in docs",
    body:
      "Marketing pages explain outcomes. Documentation covers configuration, permissions, cost receipts, providers, and code navigation internals."
  }
];

export const benchmarkRows: BenchmarkRow[] = [
  { lang: "C", squeezyCost: 0.0454, baselineCost: 0.0504, ratio: 0.9, recall: 100, verdict: "WIN" },
  { lang: "C++", squeezyCost: 0.0557, baselineCost: 0.0689, ratio: 0.81, recall: 100, verdict: "WIN" },
  { lang: "C#", squeezyCost: 0.016, baselineCost: 0.0341, ratio: 0.47, recall: 100, verdict: "WIN" },
  { lang: "Dart", squeezyCost: 0.1049, baselineCost: 0.1802, ratio: 0.58, recall: 100, verdict: "WIN" },
  { lang: "Go", squeezyCost: 0.0222, baselineCost: 0.0477, ratio: 0.47, recall: 100, verdict: "WIN" },
  { lang: "Java", squeezyCost: 0.0488, baselineCost: 0.1094, ratio: 0.45, recall: 100, verdict: "WIN" },
  { lang: "JS", squeezyCost: 0.0122, baselineCost: 0.0182, ratio: 0.67, recall: 100, verdict: "WIN" },
  { lang: "Kotlin", squeezyCost: 0.0271, baselineCost: 0.0416, ratio: 0.65, recall: 100, verdict: "WIN" },
  { lang: "PHP", squeezyCost: 0.0261, baselineCost: 0.0418, ratio: 0.62, recall: 100, verdict: "WIN" },
  { lang: "Python", squeezyCost: 0.0155, baselineCost: 0.0193, ratio: 0.81, recall: 100, verdict: "WIN" },
  { lang: "Ruby", squeezyCost: 0.0134, baselineCost: 0.0496, ratio: 0.27, recall: 100, verdict: "WIN" },
  { lang: "Rust", squeezyCost: 0.0278, baselineCost: 0.0355, ratio: 0.78, recall: 100, verdict: "WIN" },
  { lang: "Scala", squeezyCost: 0.0202, baselineCost: 0.0611, ratio: 0.33, recall: 100, verdict: "WIN" },
  { lang: "Swift", squeezyCost: 0.0134, baselineCost: 0.0181, ratio: 0.74, recall: 100, verdict: "WIN" },
  { lang: "TS", squeezyCost: 0.0378, baselineCost: 0.0424, ratio: 0.89, recall: 100, verdict: "WIN" }
];

export const haikuBenchmarkRows: BenchmarkRow[] = [
  { lang: "C", squeezyCost: 0.2494, baselineCost: 0.2474, ratio: 1.01, recall: 100, verdict: "LOSS" },
  { lang: "C++", squeezyCost: 0.1707, baselineCost: 0.2074, ratio: 0.82, recall: 100, verdict: "WIN" },
  { lang: "C#", squeezyCost: 0.2242, baselineCost: 0.2364, ratio: 0.95, recall: 100, verdict: "WIN" },
  { lang: "Dart", squeezyCost: 0.1326, baselineCost: 0.2275, ratio: 0.58, recall: 100, verdict: "WIN" },
  { lang: "Go", squeezyCost: 0.2336, baselineCost: 0.1479, ratio: 1.58, recall: 100, verdict: "LOSS" },
  { lang: "Java", squeezyCost: 0.267, baselineCost: 0.3696, ratio: 0.72, recall: 100, verdict: "WIN" },
  { lang: "JS", squeezyCost: 0.0404, baselineCost: 0.0549, ratio: 0.74, recall: 100, verdict: "WIN" },
  { lang: "Kotlin", squeezyCost: 0.1159, baselineCost: 0.2038, ratio: 0.57, recall: 100, verdict: "WIN" },
  { lang: "PHP", squeezyCost: 0.0499, baselineCost: 0.1083, ratio: 0.46, recall: 100, verdict: "WIN" },
  { lang: "Python", squeezyCost: 0.058, baselineCost: 0.1074, ratio: 0.54, recall: 100, verdict: "WIN" },
  { lang: "Ruby", squeezyCost: 0.2178, baselineCost: 0.2963, ratio: 0.73, recall: 100, verdict: "WIN" },
  { lang: "Rust", squeezyCost: 0.0858, baselineCost: 0.1509, ratio: 0.57, recall: 80, verdict: "WIN" },
  { lang: "Scala", squeezyCost: 0.1959, baselineCost: 0.2884, ratio: 0.68, recall: 100, verdict: "WIN" },
  { lang: "Swift", squeezyCost: 0.0215, baselineCost: 0.0342, ratio: 0.63, recall: 100, verdict: "WIN" },
  { lang: "TS", squeezyCost: 0.0791, baselineCost: 0.0996, ratio: 0.79, recall: 100, verdict: "WIN" }
];

export const benchmarkSummary = {
  codexWins: "15 / 15",
  claudeWins: "13 / 15",
  codexModel: "Squeezy gpt-5.4-mini vs Codex gpt-5.4-mini",
  claudeModel: "Squeezy claude-haiku-4-5 vs Claude Code haiku",
  runs: "n=3 medians",
  totalDelta: "lower model spend",
  medianRatio: "0.78",
  suite:
    "same-task real-world code-navigation benchmark, equal pricing and grader, Squeezy versus Codex on the Mini tier and Claude Code on the Haiku tier.",
  source:
    "docs/internal/eval-findings/board-and-graph-fixes-summary.md"
};

export const accuracyRows: MatrixRow[] = [
  {
    name: "Rust",
    detail:
      "Benchmarks compare local navigation output against language-specific validation oracles for declaration and relationship coverage.",
    status: "checked"
  },
  {
    name: "Java",
    detail:
      "External repositories are used to test whether Squeezy can find declarations and project relationships without relying on model guesses.",
    status: "checked"
  },
  {
    name: "Go",
    detail:
      "Refresh probes verify that changed files can be re-indexed without rebuilding the whole repository understanding from scratch.",
    status: "checked"
  }
];

export const operatingLoop: FactCard[] = [
  {
    label: "1",
    title: "Understand the repo locally",
    body:
      "Squeezy builds a local code map and workspace view so the first model call does not start from a blank repository."
  },
  {
    label: "2",
    title: "Read only the relevant code",
    body:
      "The agent narrows broad questions into specific files, symbols, diffs, command outputs, or verifier steps."
  },
  {
    label: "3",
    title: "Keep context tight",
    body:
      "Repeated output is replaced with receipts, noisy output is shaped, and long conversations are compacted before they become expensive."
  },
  {
    label: "4",
    title: "Send focused work to the model",
    body:
      "The selected provider gets the useful context, and Squeezy tracks tokens, cache usage, tool output, and estimated spend."
  }
];

export const languageRows: MatrixRow[] = [
  { name: "Rust", detail: "Cargo workspaces, crates, traits, impls, modules, and tests." },
  { name: "Python", detail: "Packages, imports, classes, functions, decorators, and inheritance." },
  { name: "Java", detail: "Packages, Maven/Gradle projects, classes, members, and inheritance." },
  { name: "Kotlin", detail: "Packages, Gradle projects, classes, objects, companions, and extensions." },
  { name: "Scala", detail: "Packages, traits, objects, case classes, enums, and extension methods." },
  { name: "C#/.NET", detail: "Solutions, namespaces, usings, partial types, attributes, and members." },
  { name: "Go", detail: "Modules, packages, structs, interfaces, receivers, imports, and tests." },
  { name: "C", detail: "Headers, includes, structs, functions, typedefs, macros, and references." },
  { name: "C++", detail: "Headers, namespaces, classes, templates, methods, and overload-heavy code." },
  { name: "JavaScript", detail: "ES modules, CommonJS, functions, classes, exports, and JSX." },
  { name: "TypeScript", detail: "Types, interfaces, imports, generics, classes, and TSX." },
  { name: "PHP", detail: "Namespaces, Composer-style code, traits, enums, attributes, and methods." },
  { name: "Ruby", detail: "Classes, modules, mixins, singleton methods, accessors, and require paths." },
  { name: "Swift", detail: "Modules, protocols, actors, structs, extensions, and property wrappers." },
  { name: "Dart", detail: "Libraries, parts, classes, mixins, extensions, and Flutter-style projects." }
];

export const providerGroups: MatrixRow[] = [
  {
    name: "Native providers",
    detail:
      "OpenAI, Anthropic, Google Gemini, Azure OpenAI, AWS Bedrock, and Ollama have dedicated or local runtime paths.",
    status: "API keys or local config"
  },
  {
    name: "Compatible APIs",
    detail:
      "OpenRouter, Vercel AI Gateway, PortKey, Groq, xAI, DeepSeek, Mistral, Together, Fireworks, Cerebras, DeepInfra, Baseten, Cloudflare Workers AI, and custom compatible endpoints.",
    status: "bring an endpoint"
  },
  {
    name: "Local runtimes",
    detail:
      "Use local or self-hosted routes such as Ollama, LM Studio, vLLM, llama.cpp-style servers, or custom compatible base URLs.",
    status: "local when configured"
  }
];

export const aggregatorRows: MatrixRow[] = [
  {
    name: "OpenRouter",
    detail: "OpenAI-compatible aggregator route with many hosted models. Pricing and cache support depend on the selected model and registry metadata.",
    status: "OPENROUTER_API_KEY"
  },
  {
    name: "Vercel AI Gateway",
    detail: "OpenAI-compatible gateway route for hosted model access through Vercel.",
    status: "AI_GATEWAY_API_KEY"
  },
  {
    name: "PortKey",
    detail: "OpenAI-compatible gateway route for virtual keys, routing, and observability.",
    status: "PORTKEY_API_KEY"
  }
];

export const providerRows: MatrixRow[] = [
  {
    name: "OpenAI",
    detail: "Native OpenAI route with usage parsing and cache-related request metadata where supported.",
    status: "OPENAI_API_KEY"
  },
  {
    name: "Anthropic",
    detail: "Native Anthropic route with API-key and OAuth credential paths plus cache read/write accounting where exposed.",
    status: "ANTHROPIC_API_KEY"
  },
  {
    name: "Google Gemini",
    detail: "Native Gemini route with API-key configuration and streaming usage metadata where available.",
    status: "GOOGLE_API_KEY"
  }
];

export const cloudPlatformRows: MatrixRow[] = [
  {
    name: "Amazon Bedrock",
    detail: "AWS-hosted provider route using the AWS credential chain and Bedrock runtime APIs.",
    status: "AWS credentials"
  },
  {
    name: "Azure OpenAI",
    detail: "Azure-hosted OpenAI route with deployment-specific endpoint and API-key or bearer-token configuration.",
    status: "AZURE_OPENAI_API_KEY"
  },
  {
    name: "Google Vertex AI",
    detail: "Google Cloud route through an OpenAI-compatible endpoint with access-token or service-account OAuth support.",
    status: "Google Cloud auth"
  }
];

export const localRuntimeRows: MatrixRow[] = [
  {
    name: "Ollama",
    detail: "Local runtime route for models served by Ollama. Context and model availability are runtime-defined.",
    status: "local runtime"
  }
];

export const openAiCompatibleRows: MatrixRow[] = [
  {
    name: "Groq, xAI, DeepSeek",
    detail: "Hosted OpenAI-compatible presets with API-key configuration and curated registry entries where available.",
    status: "API key"
  },
  {
    name: "Mistral, Together, Fireworks, Cerebras",
    detail: "OpenAI-compatible hosted inference presets. Dollar estimates require matching pricing metadata.",
    status: "API key"
  },
  {
    name: "Custom and local compatible endpoints",
    detail: "Custom OpenAI-compatible base URLs, plus local LM Studio, vLLM, and llama.cpp style routes.",
    status: "preset"
  }
];

export const installRows: MatrixRow[] = [
  {
    name: "macOS",
    detail: "Release targets for Apple Silicon and Intel. Primary path is the one-line installer; Homebrew tap support is scripted.",
    status: "aarch64 + x86_64"
  },
  {
    name: "Linux",
    detail: "x86_64 musl static binary target plus source install when the Rust toolchain is already present.",
    status: "x86_64"
  },
  {
    name: "Windows",
    detail: "x86_64 MSVC archive and Winget manifest update path. Some sandbox behavior is platform-limited on Windows.",
    status: "x86_64"
  }
];

export const supportCoverageRows: MatrixRow[] = [
  {
    name: "Operating systems",
    detail: "macOS on Apple Silicon and Intel, Linux x86_64, and Windows x86_64.",
    status: "install targets"
  },
  {
    name: "Languages",
    detail: "Rust, Python, Java, Kotlin, Scala, C#/.NET, Go, C, C++, JavaScript, TypeScript, PHP, Ruby, Swift, and Dart.",
    status: "15"
  },
  {
    name: "Providers",
    detail: "Native providers, compatible APIs, cloud hosts, OAuth-style routes, custom endpoints, and local runtimes.",
    status: "bring your model"
  },
  {
    name: "Diagnostics",
    detail: "`/feedback`, `/report`, and `squeezy sessions report` create support material you can preview before sending.",
    status: "redacted"
  }
];

export const trustRows: MatrixRow[] = [
  {
    name: "Permissions",
    detail:
      "Configurable policies cover edits, shell, web, MCP, destructive actions, and outside-workspace paths.",
    status: "reviewable"
  },
  {
    name: "Website analytics",
    detail:
      "Website visits and clicks are sent through the configured telemetry endpoint so the site can be improved.",
    status: "PostHog"
  },
  {
    name: "Rollback",
    detail:
      "Optional checkpoints can record mutating tool calls and support call-level or turn-level rollback when configured.",
    status: "optional"
  }
];
