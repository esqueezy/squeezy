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
