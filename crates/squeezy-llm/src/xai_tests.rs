use super::{XaiRoute, classify_route, is_responses_capable};

#[test]
fn xai_uses_responses_api_for_grok_3_and_newer() {
    // Grok 3 onward exposes the OpenAI-Responses-compatible endpoint; the
    // routing predicate must select Responses for every supported variant
    // so callers do not silently degrade to Chat Completions.
    let responses_models = [
        "grok-3",
        "grok-3-mini",
        "grok-3-fast",
        "grok-4",
        "grok-4-fast-reasoning",
        "grok-4-fast-non-reasoning",
        "grok-code-fast-1",
        "GROK-4",
    ];
    for model in responses_models {
        assert!(
            is_responses_capable(model),
            "{model} must route via Responses API"
        );
    }
}

#[test]
fn xai_uses_chat_completions_for_grok_2_and_earlier() {
    // grok-2 / grok-beta / grok-1 predate the Responses launch and only
    // answer Chat Completions. Mis-routing them onto Responses would 404
    // every turn, so the predicate must return false.
    let chat_models = [
        "grok-2",
        "grok-2-mini",
        "grok-2-vision",
        "grok-beta",
        "grok-1",
    ];
    for model in chat_models {
        assert!(
            !is_responses_capable(model),
            "{model} must route via Chat Completions"
        );
    }
}

#[test]
fn xai_routes_unknown_grok_generations_to_responses() {
    // xAI treats Responses as the canonical surface as of the May 2026
    // catalog refresh: any unrecognised `grok-…` SKU must default to
    // Responses so future generations work without a code change.
    assert!(is_responses_capable("grok-5"));
    assert!(is_responses_capable("grok-5-mini"));
    assert!(is_responses_capable("grok-omega-2027"));
}

#[test]
fn xai_routes_non_grok_ids_to_chat_completions() {
    // Defensive fallback: non-grok ids and empty strings stay on Chat
    // Completions because that endpoint accepts arbitrary model strings
    // a user might have routed through a base_url override.
    assert!(!is_responses_capable(""));
    assert!(!is_responses_capable("not-a-grok"));
    // `grok-` with no generation suffix is ambiguous; route to Responses
    // because it lands under the "unknown grok" branch.
    assert!(is_responses_capable("grok-"));
}

#[test]
fn xai_strips_aggregator_namespace_prefix() {
    // A `vendor/model` prefix appears when a model id is forwarded from an
    // aggregator (OpenRouter, Vercel AI Gateway) but the caller pointed the
    // xAI provider at a base_url that still serves the vendor route. Honour
    // the namespace so routing tracks the underlying generation.
    assert!(is_responses_capable("xai/grok-4"));
    assert!(!is_responses_capable("xai/grok-2"));
}

#[test]
fn xai_classify_route_covers_new_grok_families_c09() {
    // C-09: explicit allow-list of Grok families xAI ships on Responses
    // as of the May 2026 catalog refresh. The parser must classify each
    // family correctly even for dotted minor versions and date-stamped
    // SKUs that the legacy digit-range matcher could not express.
    let responses_models = [
        "grok-4.3",
        "grok-4.3-0309",
        "grok-4.20-multi-agent-0309",
        "grok-4.20-0309-reasoning",
        "grok-4.20-0309-non-reasoning",
        "grok-build-0.1",
        "grok-build-1.0-256k",
        "grok-code-fast-1",
    ];
    for model in responses_models {
        assert_eq!(
            classify_route(model),
            XaiRoute::Responses,
            "{model} must classify as Responses"
        );
    }
}

#[test]
fn xai_classify_route_rejects_imagine_family_c09() {
    // C-09: `grok-imagine-*` is image-only and lives on
    // `/v1/images/generations`. Neither sub-provider knows that
    // endpoint, so the dispatcher must surface a structured rejection
    // rather than route to chat (where the parser would 404).
    let imagine_models = [
        "grok-imagine",
        "grok-imagine-image",
        "grok-imagine-1",
        "GROK-IMAGINE-IMAGE",
    ];
    for model in imagine_models {
        assert_eq!(
            classify_route(model),
            XaiRoute::ImageNotRouted,
            "{model} must classify as ImageNotRouted"
        );
        assert!(
            !is_responses_capable(model),
            "{model} must not be classified as Responses-capable"
        );
    }
}
