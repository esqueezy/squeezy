export const SITE = {
  name: "Squeezy",
  url: "https://squeezyagent.com",
  description:
    "A Rust coding agent that spends model tokens only after local static analysis has narrowed the code evidence.",
  repoUrl: "https://github.com/esqueezy/squeezy",
  issuesUrl: "https://github.com/esqueezy/squeezy/issues",
  discussionsUrl: "https://github.com/esqueezy/squeezy/discussions",
  telemetryEndpoint: "https://squeezy-telemetry.esqueezy.workers.dev/v1/site",
  securityContactLabel: "security contact before public v0"
};

export const DOCS_NAV = [
  {
    href: "/docs/install/",
    label: "Install",
    status: "setup"
  },
  {
    href: "/docs/semantic-navigation/",
    label: "Graph",
    status: "static analysis"
  },
  {
    href: "/docs/cost-receipts/",
    label: "Optimization",
    status: "token budget"
  },
  {
    href: "/docs/config/",
    label: "Config",
    status: "settings"
  },
  {
    href: "/docs/permissions/",
    label: "Permissions",
    status: "policy"
  },
  {
    href: "/docs/languages/",
    label: "Languages",
    status: "coverage"
  },
  {
    href: "/docs/providers/",
    label: "Providers",
    status: "models"
  },
  {
    href: "/docs/troubleshooting/",
    label: "Support",
    status: "debug/report"
  }
];
