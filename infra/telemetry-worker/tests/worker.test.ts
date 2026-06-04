import { afterEach, expect, test } from "bun:test";
import worker from "../src/worker";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

function env() {
  return {
    POSTHOG_PROJECT_TOKEN: "test-token",
    POSTHOG_HOST: "https://eu.i.posthog.com",
  };
}

test("product telemetry accepts current trace and routing properties", async () => {
  const forwarded: unknown[] = [];
  globalThis.fetch = async (_input: RequestInfo | URL, init?: RequestInit) => {
    forwarded.push(JSON.parse(String(init?.body)));
    return new Response(null, { status: 200 });
  };

  const response = await worker.fetch(
    new Request("https://telemetry.example/v1/batch", {
      method: "POST",
      body: JSON.stringify({
        schema_version: 1,
        user_id: "11111111-1111-4111-8111-111111111111",
        install_id: "11111111-1111-4111-8111-111111111111",
        session_id: "22222222-2222-4222-8222-222222222222",
        app_version: "0.1.0",
        os: "macos",
        arch: "aarch64",
        events: [
          {
            event: "squeezy_tool_completed",
            timestamp_ms: Date.now(),
            event_sequence: 1,
            properties: {
              provider: "port_key",
              model_family: "other",
              tool_name: "shell",
              tool_family: "shell",
              tool_status: "success",
              duration_ms: 12,
              args_sha256: "a".repeat(64),
              output_sha256: "b".repeat(64),
              content_sha256: "c".repeat(64),
              trace_id: "d".repeat(32),
              span_id: "e".repeat(16),
            },
          },
          {
            event: "squeezy_routing_routed",
            timestamp_ms: Date.now(),
            event_sequence: 2,
            properties: {
              routing_reason: "llm_judge",
              trace_id: "d".repeat(32),
            },
          },
          {
            event: "approval_best_effort_fallback",
            timestamp_ms: Date.now(),
            event_sequence: 3,
            properties: {
              tool_name: "shell",
              tool_family: "shell",
              sandbox_backend: "macos-sandbox-exec",
              trace_id: "d".repeat(32),
            },
          },
          {
            event: "squeezy_config_change_committed",
            timestamp_ms: Date.now(),
            event_sequence: 4,
            properties: {
              config_scope: "project",
              config_section: "models",
              config_field: "model.model",
              config_apply_tier: "next_prompt",
              config_change_kind: "set",
              config_prev_bucket: "model_custom",
              config_new_bucket: "model_custom",
              local_path: "/Users/example/project",
            },
          },
          {
            event: "squeezy_startup_ready",
            timestamp_ms: Date.now(),
            event_sequence: 5,
            properties: {
              startup_route: "resume_picker_resume",
              duration_ms: 987,
              status: "success",
            },
          },
          {
            event: "squeezy_slash_command_used",
            timestamp_ms: Date.now(),
            event_sequence: 6,
            properties: {
              slash_command: "plan",
              slash_surface: "tui_composer",
              slash_outcome: "accepted",
              slash_alias_kind: "canonical",
              slash_arg_shape: "free_text",
            },
          },
          {
            event: "squeezy_session_ended",
            timestamp_ms: Date.now(),
            event_sequence: 7,
            properties: {
              session_status: "completed",
              duration_ms: 1234,
              turn_count: 2,
              tool_successes: 3,
              tool_errors: 1,
              tool_denials: 0,
              tool_cancellations: 0,
              subagent_calls: 1,
              subagent_failures: 0,
            },
          },
        ],
      }),
    }),
    env(),
  );

  expect(response.status).toBe(204);
  expect(forwarded).toHaveLength(1);
  const batch = forwarded[0] as { batch: Array<{ event: string; properties: Record<string, unknown> }> };
  expect(batch.batch.map((event) => event.event)).toEqual([
    "squeezy_tool_completed",
    "squeezy_routing_routed",
    "approval_best_effort_fallback",
    "squeezy_config_change_committed",
    "squeezy_startup_ready",
    "squeezy_slash_command_used",
    "squeezy_session_ended",
  ]);
  expect(batch.batch[0].properties.trace_id).toBe("d".repeat(32));
  expect(batch.batch[3].properties.config_new_bucket).toBe("model_custom");
  expect(batch.batch[3].properties.local_path).toBeUndefined();
  expect(batch.batch[4].properties.startup_route).toBe("resume_picker_resume");
  expect(batch.batch[5].properties.slash_command).toBe("plan");
  expect(batch.batch[6].properties.session_status).toBe("completed");
});

test("product telemetry drops unknown or malformed optional properties", async () => {
  const forwarded: unknown[] = [];
  globalThis.fetch = async (_input: RequestInfo | URL, init?: RequestInit) => {
    forwarded.push(JSON.parse(String(init?.body)));
    return new Response(null, { status: 200 });
  };

  const response = await worker.fetch(
    new Request("https://telemetry.example/v1/batch", {
      method: "POST",
      body: JSON.stringify({
        schema_version: 1,
        user_id: "11111111-1111-4111-8111-111111111111",
        install_id: "11111111-1111-4111-8111-111111111111",
        session_id: "22222222-2222-4222-8222-222222222222",
        app_version: "0.1.0",
        os: "macos",
        arch: "aarch64",
        events: [
          {
            event: "squeezy_future_counter",
            timestamp_ms: Date.now(),
            event_sequence: 1,
            properties: {
              provider: "open_ai",
              model_family: "gpt",
              tool_status: "future_status",
              trace_id: "not-a-trace",
              local_path: "/Users/example/project",
              future_counter: 123,
            },
          },
        ],
      }),
    }),
    env(),
  );

  expect(response.status).toBe(204);
  expect(forwarded).toHaveLength(1);
  const batch = forwarded[0] as { batch: Array<{ event: string; properties: Record<string, unknown> }> };
  expect(batch.batch[0].event).toBe("squeezy_future_counter");
  expect(batch.batch[0].properties.provider).toBe("open_ai");
  expect(batch.batch[0].properties.model_family).toBe("gpt");
  expect(batch.batch[0].properties.tool_status).toBeUndefined();
  expect(batch.batch[0].properties.trace_id).toBeUndefined();
  expect(batch.batch[0].properties.local_path).toBeUndefined();
  expect(batch.batch[0].properties.future_counter).toBeUndefined();
});

test("site telemetry accepts page view and forwards sanitized properties", async () => {
  const forwarded: unknown[] = [];
  globalThis.fetch = async (_input: RequestInfo | URL, init?: RequestInit) => {
    forwarded.push(JSON.parse(String(init?.body)));
    return new Response(null, { status: 200 });
  };

  const response = await worker.fetch(
    new Request("https://telemetry.example/v1/site", {
      method: "POST",
      body: JSON.stringify({
        schema_version: 1,
        visitor_id: "11111111-1111-4111-8111-111111111111",
        session_id: "22222222-2222-4222-8222-222222222222",
        timestamp_ms: Date.now(),
        event: "squeezy_site_page_view",
        path: "/languages/",
        referrer_kind: "internal",
        utm_source: "docs",
      }),
    }),
    env(),
  );

  expect(response.status).toBe(204);
  expect(forwarded).toHaveLength(1);
  const batch = forwarded[0] as { batch: Array<{ event: string; properties: Record<string, unknown> }> };
  expect(batch.batch[0].event).toBe("squeezy_site_page_view");
  expect(batch.batch[0].properties.distinct_id).toBe("11111111-1111-4111-8111-111111111111");
  expect(batch.batch[0].properties.path).toBe("/languages/");
  expect(batch.batch[0].properties.utm_source).toBe("docs");
});

test("site telemetry rejects unknown fields", async () => {
  let called = false;
  globalThis.fetch = async () => {
    called = true;
    return new Response(null, { status: 200 });
  };

  const response = await worker.fetch(
    new Request("https://telemetry.example/v1/site", {
      method: "POST",
      body: JSON.stringify({
        schema_version: 1,
        visitor_id: "11111111-1111-4111-8111-111111111111",
        session_id: "22222222-2222-4222-8222-222222222222",
        timestamp_ms: Date.now(),
        event: "squeezy_site_page_view",
        path: "/",
        referrer_kind: "none",
        raw_url: "https://squeezyagent.com/?secret=1",
      }),
    }),
    env(),
  );

  expect(response.status).toBe(400);
  expect(called).toBe(false);
});

test("site telemetry handles cors preflight", async () => {
  const response = await worker.fetch(new Request("https://telemetry.example/v1/site", { method: "OPTIONS" }), env());

  expect(response.status).toBe(204);
  expect(response.headers.get("access-control-allow-origin")).toBe("https://squeezyagent.com");
  expect(response.headers.get("access-control-allow-methods")).toContain("POST");
});
