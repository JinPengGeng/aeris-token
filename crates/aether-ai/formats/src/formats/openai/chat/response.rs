use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::{
    formats::{
        context::FormatContext, openai::responses::response::openai_responses_current_timestamp,
    },
    protocol::canonical::{
        canonical_blocks_to_openai_chat_message, canonical_stop_reason_to_openai,
        canonical_usage_to_openai, openai_extensions, openai_finish_reason_to_canonical,
        openai_message_content_blocks, openai_service_tier_extension, openai_usage_to_canonical,
        CanonicalContentBlock, CanonicalResponse, CanonicalResponseOutput, CanonicalRole,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
};

/// Reasoning text carried by one Chat Completions `message` or streaming
/// `delta`, paired with the provider's reasoning block index where one exists.
///
/// The field name is not standardized.  DeepSeek-style upstreams send
/// `reasoning_content`; OpenRouter sends `reasoning` alongside a structured
/// `reasoning_details` array.  OpenRouter repeats the same text in both of its
/// fields, so exactly one source is read per object and `reasoning_details`
/// wins because only it carries the block index.
pub(crate) fn openai_chat_reasoning_texts(
    object: &Map<String, Value>,
) -> Vec<(Option<usize>, String)> {
    if let Some(details) = object.get("reasoning_details").and_then(Value::as_array) {
        let texts = details
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|detail| {
                // `reasoning.encrypted` carries opaque provider state rather
                // than readable text, so it has nothing to hand downstream.
                if detail.get("type").and_then(Value::as_str) == Some("reasoning.encrypted") {
                    return None;
                }
                let text = ["text", "summary"].iter().find_map(|key| {
                    detail
                        .get(*key)
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                })?;
                let index = detail
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|index| index as usize);
                Some((index, text.to_string()))
            })
            .collect::<Vec<_>>();
        if !texts.is_empty() {
            return texts;
        }
    }
    // A provider may null out one spelling while filling the other, so skip
    // past any key that is present but carries no string.
    ["reasoning_content", "reasoning"]
        .iter()
        .find_map(|key| {
            object
                .get(*key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
        })
        .map(|text| vec![(None, text.to_string())])
        .unwrap_or_default()
}

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalResponse> {
    from_raw(body)
}

pub fn to(response: &CanonicalResponse, _ctx: &FormatContext) -> Option<Value> {
    Some(to_raw(response))
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalResponse> {
    let body = body_json.as_object()?;
    if body.contains_key("error") {
        return None;
    }
    let mut outputs = Vec::new();
    for (fallback_index, choice_value) in body
        .get("choices")
        .and_then(Value::as_array)?
        .iter()
        .enumerate()
    {
        let choice = choice_value.as_object()?;
        let message = choice.get("message").and_then(Value::as_object)?;
        let mut content = openai_message_content_blocks(message)?;
        if !content
            .iter()
            .any(|block| matches!(block, CanonicalContentBlock::Thinking { .. }))
        {
            let thinking = openai_chat_reasoning_texts(message)
                .into_iter()
                .map(|(_, text)| text)
                .filter(|text| !text.trim().is_empty())
                .map(|text| CanonicalContentBlock::Thinking {
                    text,
                    signature: None,
                    encrypted_content: None,
                    extensions: BTreeMap::new(),
                })
                .collect::<Vec<_>>();
            content.splice(0..0, thinking);
        }
        let stop_reason =
            openai_finish_reason_to_canonical(choice.get("finish_reason").and_then(Value::as_str));
        let mut extensions = BTreeMap::new();
        if let Some(raw_finish_reason) = choice.get("finish_reason").cloned() {
            extensions.insert(
                "openai".to_string(),
                json!({ "raw_finish_reason": raw_finish_reason }),
            );
        }
        outputs.push(CanonicalResponseOutput {
            index: choice
                .get("index")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(fallback_index),
            role: CanonicalRole::Assistant,
            content,
            stop_reason,
            extensions,
        });
    }
    let first_output = outputs.first()?;
    let content = first_output.content.clone();
    let stop_reason = first_output.stop_reason.clone();
    Some(CanonicalResponse {
        id: body
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("chatcmpl-unknown")
            .to_string(),
        model: body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        outputs,
        content,
        stop_reason,
        usage: openai_usage_to_canonical(body.get("usage")),
        extensions: openai_extensions(
            body,
            &["id", "object", "model", "choices", "usage", "created"],
        ),
    })
}

pub fn to_raw(canonical: &CanonicalResponse) -> Value {
    let outputs: Vec<CanonicalResponseOutput> = if canonical.outputs.is_empty() {
        vec![CanonicalResponseOutput {
            index: 0,
            role: CanonicalRole::Assistant,
            content: canonical.content.clone(),
            stop_reason: canonical.stop_reason.clone(),
            extensions: BTreeMap::new(),
        }]
    } else {
        canonical.outputs.clone()
    };
    let choices: Vec<Value> = outputs
        .iter()
        .enumerate()
        .map(|(fallback_index, output)| {
            let finish_reason = output
                .extensions
                .get("openai")
                .and_then(Value::as_object)
                .and_then(|openai| openai.get("raw_finish_reason"))
                .cloned()
                .unwrap_or_else(|| {
                    Value::String(
                        canonical_stop_reason_to_openai(output.stop_reason.as_ref()).to_string(),
                    )
                });
            json!({
                "index": output.index,
                "message": canonical_blocks_to_openai_chat_message(&output.content),
                "finish_reason": finish_reason,
            })
            .as_object()
            .map(|choice| {
                let mut choice = choice.clone();
                if output.index == 0 && fallback_index != 0 {
                    choice.insert("index".to_string(), Value::from(fallback_index as u64));
                }
                Value::Object(choice)
            })
            .unwrap_or_else(|| json!({}))
        })
        .collect();

    let mut response = json!({
        "id": canonical.id,
        "object": "chat.completion",
        "model": canonical.model,
        "choices": choices,
        "usage": canonical.usage.as_ref().map(canonical_usage_to_openai).unwrap_or_else(|| json!({
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
        })),
    });
    // Canonical responses converted from other formats (claude, gemini, ...)
    // carry no `created_at` extension, so fall back to the current unix time:
    // strict OpenAI clients treat `created` as required on `chat.completion`.
    // An upstream-provided extension value always wins over the fallback.
    let created_at = canonical
        .extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            canonical
                .extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .and_then(|value| value.get("created_at"))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().map(|value| value as i64))
        })
        .unwrap_or_else(openai_responses_current_timestamp);
    response["created"] = Value::from(created_at);
    if let Some(service_tier) = openai_service_tier_extension(&canonical.extensions).cloned() {
        response["service_tier"] = service_tier;
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::canonical::CanonicalContentBlock;

    fn thinking_texts(response: &CanonicalResponse) -> Vec<String> {
        response
            .content
            .iter()
            .filter_map(|block| match block {
                CanonicalContentBlock::Thinking { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn empty_reasoning_aliases_fall_through_to_readable_text() {
        for fields in [
            json!({"reasoning_content": "", "reasoning": "kept"}),
            json!({"reasoning_content": null, "reasoning": "kept"}),
            json!({"reasoning_details": [{"type": "reasoning.summary", "text": "", "summary": "kept"}]}),
            json!({"reasoning_details": [{"type": "reasoning.summary", "text": null, "summary": "kept"}]}),
        ] {
            let mut message = fields;
            message["role"] = json!("assistant");
            message["content"] = json!("done");
            let response = from_raw(&json!({"choices": [{"index": 0, "message": message,
                "finish_reason": "stop"}]}))
            .expect("response");
            assert_eq!(thinking_texts(&response), vec!["kept"]);
        }
    }
    #[test]
    fn openrouter_reasoning_details_become_thinking_blocks() {
        let response = from_raw(&json!({
            "id": "gen-openrouter-123",
            "model": "stealth/ox-alpha",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "done",
                    "reasoning": "step onestep two",
                    "reasoning_details": [
                        {"type": "reasoning.text", "text": "step one", "index": 0},
                        {"type": "reasoning.text", "text": "step two", "index": 1}
                    ]
                },
                "finish_reason": "stop"
            }]
        }))
        .expect("openrouter response should convert");

        // `reasoning` repeats the same text the details already carry, so the
        // details win and the provider's own segmentation survives.
        assert_eq!(thinking_texts(&response), vec!["step one", "step two"]);
    }

    #[test]
    fn openrouter_reasoning_string_becomes_a_thinking_block() {
        let response = from_raw(&json!({
            "id": "gen-openrouter-123",
            "model": "stealth/ox-alpha",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "done",
                    "reasoning": "thought about it"
                },
                "finish_reason": "stop"
            }]
        }))
        .expect("openrouter response should convert");

        assert_eq!(thinking_texts(&response), vec!["thought about it"]);
    }

    #[test]
    fn deepseek_reasoning_content_still_becomes_a_thinking_block() {
        let response = from_raw(&json!({
            "id": "chatcmpl-deepseek",
            "model": "deepseek-reasoner",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "42",
                    "reasoning_content": "let me work it out"
                },
                "finish_reason": "stop"
            }]
        }))
        .expect("deepseek response should convert");

        assert_eq!(thinking_texts(&response), vec!["let me work it out"]);
    }

    #[test]
    fn deepseek_reasoning_content_wins_over_a_bare_reasoning_field() {
        let response = from_raw(&json!({
            "id": "chatcmpl-deepseek",
            "model": "deepseek-reasoner",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "42",
                    "reasoning_content": "the real one",
                    "reasoning": "the other spelling"
                },
                "finish_reason": "stop"
            }]
        }))
        .expect("deepseek response should convert");

        assert_eq!(thinking_texts(&response), vec!["the real one"]);
    }

    #[test]
    fn blank_reasoning_content_produces_no_thinking_block() {
        let response = from_raw(&json!({
            "id": "chatcmpl-deepseek",
            "model": "deepseek-reasoner",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "42", "reasoning_content": "   "},
                "finish_reason": "stop"
            }]
        }))
        .expect("deepseek response should convert");

        assert!(thinking_texts(&response).is_empty());
    }

    #[test]
    fn plain_openai_response_without_reasoning_is_unchanged() {
        let response = from_raw(&json!({
            "id": "chatcmpl-openai",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }]
        }))
        .expect("openai response should convert");

        assert!(thinking_texts(&response).is_empty());
    }

    #[test]
    fn encrypted_reasoning_details_carry_no_thinking_text() {
        let response = from_raw(&json!({
            "id": "gen-openrouter-123",
            "model": "stealth/ox-alpha",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "done",
                    "reasoning_details": [
                        {"type": "reasoning.encrypted", "data": "b3BhcXVl", "index": 0}
                    ]
                },
                "finish_reason": "stop"
            }]
        }))
        .expect("openrouter response should convert");

        assert!(thinking_texts(&response).is_empty());
    }

    fn unix_now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default()
    }

    fn minimal_canonical() -> CanonicalResponse {
        from_raw(&json!({
            "id": "chatcmpl-created-123",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "done"},
                "finish_reason": "stop"
            }]
        }))
        .expect("response should convert")
    }

    /// 跨格式 canonical（claude/gemini 等来源）没有 `created_at` 扩展：
    /// `to_raw` 必须回退当前 unix 秒，否则严格客户端反序列化 `chat.completion`
    /// 会报 "missing field `created`"（fork issue #569 / 上游 #738）。
    #[test]
    fn to_raw_falls_back_to_current_unix_seconds_without_created_at() {
        let canonical = minimal_canonical();
        let before = unix_now_secs();
        let raw = to_raw(&canonical);
        let created = raw["created"]
            .as_i64()
            .expect("created must be present on chat.completion");
        assert!(
            created >= before && created <= unix_now_secs(),
            "created {created} must be a plausible unix second"
        );
    }

    /// 透传对照：canonical 已带 `created_at` 扩展时保留上游原值，不被回退覆盖。
    #[test]
    fn to_raw_keeps_created_at_extension_value() {
        let mut canonical = minimal_canonical();
        canonical.extensions.insert(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            json!({"created_at": 1_770_000_000i64}),
        );
        let raw = to_raw(&canonical);
        assert_eq!(raw["created"], 1_770_000_000i64);
    }
}
