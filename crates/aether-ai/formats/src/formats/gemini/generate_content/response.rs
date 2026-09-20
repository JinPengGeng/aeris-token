use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::{
    formats::context::FormatContext,
    formats::shared::citations::{
        canonical_citation, canonical_citations_to_claude_citations,
        canonical_citations_to_openai_annotations,
    },
    protocol::canonical::{
        canonical_extension_object_mut, canonical_usage_total_input_tokens,
        canonical_usage_total_tokens_for_inclusive_input, gemini_extensions,
        gemini_part_to_canonical_block, gemini_stop_reason_to_canonical, gemini_usage_to_canonical,
        CanonicalContentBlock, CanonicalResponse, CanonicalResponseOutput, CanonicalRole,
        CanonicalStopReason, CanonicalUsage, CLAUDE_EXTENSION_NAMESPACE,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
    },
};

/// Project Gemini grounding metadata onto the answer text as structured
/// citations.
///
/// Native `googleSearch` grounding runs inside Google, so there is no
/// client-visible tool call and the evidence only exists in
/// `candidates[].groundingMetadata`. Cross-format targets used to drop that
/// wholesale, leaving callers with prose that names its sources but nothing a
/// client can render or verify. Every grounded span is therefore emitted twice,
/// each time in the target family's own standard shape: OpenAI `url_citation`
/// annotations and Claude `web_search_result_location` citations. Both ride
/// extension namespaces the respective emitters already merge onto the text
/// block, so no target has to learn anything Gemini-specific.
fn attach_gemini_grounding_citations(
    candidate: &Map<String, Value>,
    content: &mut [(usize, CanonicalContentBlock)],
) {
    let Some(grounding) = gemini_candidate_grounding(candidate) else {
        return;
    };
    let texts = content
        .iter()
        .filter_map(|(index, block)| {
            let CanonicalContentBlock::Text { text, .. } = block else {
                return None;
            };
            let mut part = GeminiCitationText::default();
            part.append(text, 0);
            Some((*index, part))
        })
        .collect();
    let mut citations_by_part: BTreeMap<usize, Vec<Value>> = BTreeMap::new();
    for (index, citation) in gemini_grounding_citations(grounding, &texts) {
        citations_by_part.entry(index).or_default().push(citation);
    }
    for (index, block) in content {
        let Some(citations) = citations_by_part.remove(index) else {
            continue;
        };
        let CanonicalContentBlock::Text { extensions, .. } = block else {
            continue;
        };
        let annotations = canonical_citations_to_openai_annotations(&citations);
        let claude_citations = canonical_citations_to_claude_citations(&citations);
        canonical_extension_object_mut(extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE)
            .entry("annotations".to_string())
            .or_insert_with(|| Value::Array(annotations));
        canonical_extension_object_mut(extensions, CLAUDE_EXTENSION_NAMESPACE)
            .entry("citations".to_string())
            .or_insert_with(|| Value::Array(claude_citations));
    }
}

/// Source part text plus runs mapping its characters to target answer offsets.
/// Sync uses block-local offsets; streams use offsets in actual TextDelta order.
#[derive(Default)]
pub(crate) struct GeminiCitationText {
    text: String,
    characters: usize,
    // (source character start, source character end, target character start)
    runs: Vec<(usize, usize, usize)>,
}

impl GeminiCitationText {
    pub(crate) fn append(&mut self, delta: &str, target_start: usize) {
        let count = delta.chars().count();
        if count == 0 {
            return;
        }
        if let Some(run) = self
            .runs
            .last_mut()
            .filter(|run| run.2 + run.1 - run.0 == target_start)
        {
            run.1 += count;
        } else {
            self.runs
                .push((self.characters, self.characters + count, target_start));
        }
        self.characters += count;
        self.text.push_str(delta);
    }

    fn segment(&self, segment: &Value) -> Option<(usize, usize, &str)> {
        let start = match segment.get("startIndex") {
            None => 0,
            Some(value) => usize::try_from(value.as_u64()?).ok()?,
        };
        let supplied_text = segment.get("text").and_then(Value::as_str);
        let end = match segment.get("endIndex") {
            Some(value) => usize::try_from(value.as_u64()?).ok()?,
            None => start.checked_add(supplied_text?.len())?,
        };
        if start >= end {
            return None;
        }
        let cited_text = self.text.get(start..end)?;
        if supplied_text.is_some_and(|text| text != cited_text) {
            return None;
        }
        let start_char = self.text[..start].chars().count();
        let end_char = start_char + cited_text.chars().count();
        // A source span crossing interleaved parts cannot be represented by a
        // single target interval. Drop it rather than include unrelated text.
        let (source_start, _, target_start) = self
            .runs
            .iter()
            .find(|(begin, end, _)| *begin <= start_char && end_char <= *end)?;
        let target_start = target_start + start_char - source_start;
        Some((
            target_start,
            target_start + end_char - start_char,
            cited_text,
        ))
    }
}

pub(crate) fn gemini_candidate_grounding(candidate: &Map<String, Value>) -> Option<&Value> {
    candidate
        .get("groundingMetadata")
        .or_else(|| candidate.get("grounding_metadata"))
}

/// Normalise provider byte offsets against their original part, then map them
/// to target character offsets. Invalid spans are never clamped onto other text.
pub(crate) fn gemini_grounding_citations(
    grounding: &Value,
    texts: &BTreeMap<usize, GeminiCitationText>,
) -> Vec<(usize, Value)> {
    let chunks = grounding
        .get("groundingChunks")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let supports = grounding
        .get("groundingSupports")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut citations = Vec::new();
    for support in supports {
        let Some(segment) = support.get("segment") else {
            continue;
        };
        let index = match segment.get("partIndex") {
            None => 0,
            Some(value) => {
                let Some(index) = value.as_u64().and_then(|value| usize::try_from(value).ok())
                else {
                    continue;
                };
                index
            }
        };
        let Some((start, end, cited_text)) =
            texts.get(&index).and_then(|text| text.segment(segment))
        else {
            continue;
        };
        let indices = support
            .get("groundingChunkIndices")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for chunk_index in indices {
            let Some(chunk) = chunk_index
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| chunks.get(index))
            else {
                continue;
            };
            let Some((uri, title)) = gemini_grounding_chunk_source(chunk) else {
                continue;
            };
            citations.push((
                index,
                canonical_citation(uri, title, Some(start), Some(end), Some(cited_text)),
            ));
        }
    }
    // Only genuinely unanchored metadata gets this fallback. Failed anchored
    // supports must not be silently rebound to a different visible part.
    if supports.is_empty() {
        if let Some((&index, _)) = texts.iter().find(|(_, text)| !text.text.trim().is_empty()) {
            for chunk in chunks {
                let Some((uri, title)) = gemini_grounding_chunk_source(chunk) else {
                    continue;
                };
                citations.push((index, canonical_citation(uri, title, None, None, None)));
            }
        }
    }
    citations
}

fn gemini_grounding_chunk_source(chunk: &Value) -> Option<(&str, Option<&str>)> {
    let source = chunk.get("web").or_else(|| chunk.get("retrievedContext"))?;
    let uri = source
        .get("uri")
        .or_else(|| source.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|uri| !uri.is_empty())?;
    let title = source
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty());
    Some((uri, title))
}

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalResponse> {
    from_raw(body)
}

pub fn to(response: &CanonicalResponse, ctx: &FormatContext) -> Option<Value> {
    to_raw(response, &ctx.report_context_value())
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalResponse> {
    let body = body_json.as_object()?;
    if body.contains_key("error") {
        return None;
    }

    let candidates = body.get("candidates")?.as_array()?;
    let usage = gemini_usage_to_canonical(body.get("usageMetadata"));
    let mut outputs = Vec::new();
    for (fallback_index, candidate) in candidates.iter().enumerate() {
        let candidate_object = candidate.as_object()?;
        let parts = candidate_object
            .get("content")
            .and_then(Value::as_object)
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut content = parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                gemini_part_to_canonical_block(part, index).map(|block| (index, block))
            })
            .collect::<Vec<_>>();
        attach_gemini_grounding_citations(candidate_object, &mut content);
        let content = content
            .into_iter()
            .map(|(_, block)| block)
            .collect::<Vec<_>>();
        let mut stop_reason = candidate_object
            .get("finishReason")
            .or_else(|| candidate_object.get("finish_reason"))
            .and_then(Value::as_str)
            .and_then(gemini_stop_reason_to_canonical);
        if content
            .iter()
            .any(|block| matches!(block, CanonicalContentBlock::ToolUse { .. }))
            && stop_reason
                .as_ref()
                .is_none_or(|reason| matches!(reason, CanonicalStopReason::EndTurn))
        {
            stop_reason = Some(CanonicalStopReason::ToolUse);
        }
        let mut extensions = gemini_extensions(
            candidate_object,
            &["index", "content", "finishReason", "finish_reason"],
        );
        if let Some(raw_finish_reason) = candidate_object
            .get("finishReason")
            .or_else(|| candidate_object.get("finish_reason"))
            .cloned()
        {
            canonical_extension_object_mut(&mut extensions, "gemini")
                .insert("raw_finish_reason".to_string(), raw_finish_reason);
        }
        outputs.push(CanonicalResponseOutput {
            index: candidate_object
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(fallback_index),
            role: CanonicalRole::Assistant,
            content,
            stop_reason,
            extensions,
        });
    }
    outputs.retain(|output| {
        gemini_response_output_has_visible_content(output)
            || gemini_response_output_is_reasoning_exhausted_terminal(output, usage.as_ref())
    });
    if outputs.is_empty() {
        return None;
    }
    let content = outputs
        .first()
        .map(|output| output.content.clone())
        .unwrap_or_default();
    let stop_reason = outputs
        .first()
        .and_then(|output| output.stop_reason.clone());

    let mut canonical = CanonicalResponse {
        id: body
            .get("responseId")
            .or_else(|| body.get("_v1internal_response_id"))
            .and_then(Value::as_str)
            .unwrap_or("gemini-local-finalize")
            .to_string(),
        model: body
            .get("modelVersion")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        outputs,
        content,
        stop_reason,
        usage,
        extensions: gemini_extensions(
            body,
            &[
                "responseId",
                "_v1internal_response_id",
                "modelVersion",
                "candidates",
                "usageMetadata",
            ],
        ),
    };
    if let Some(candidates) = body.get("candidates").cloned() {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("raw_candidates".to_string(), candidates);
    }
    Some(canonical)
}

fn gemini_response_output_has_visible_content(output: &CanonicalResponseOutput) -> bool {
    output.content.iter().any(|block| match block {
        CanonicalContentBlock::Text { text, .. } | CanonicalContentBlock::Thinking { text, .. } => {
            !text.trim().is_empty()
        }
        CanonicalContentBlock::ToolUse { .. }
        | CanonicalContentBlock::ToolResult { .. }
        | CanonicalContentBlock::Image { .. }
        | CanonicalContentBlock::File { .. }
        | CanonicalContentBlock::Audio { .. } => true,
        CanonicalContentBlock::Unknown { .. } => false,
    })
}

fn gemini_response_output_is_reasoning_exhausted_terminal(
    output: &CanonicalResponseOutput,
    usage: Option<&CanonicalUsage>,
) -> bool {
    matches!(output.stop_reason, Some(CanonicalStopReason::MaxTokens))
        && usage.is_some_and(|usage| usage.reasoning_tokens > 0)
        && output.content.iter().any(|block| {
            matches!(
                block,
                CanonicalContentBlock::Thinking {
                    text,
                    signature: Some(signature),
                    ..
                } if text.trim().is_empty() && !signature.trim().is_empty()
            )
        })
}

pub fn to_raw(canonical: &CanonicalResponse, report_context: &Value) -> Option<Value> {
    let mut response = canonical_to_gemini_response(canonical, report_context)?;
    if let Some(object) = response.as_object_mut() {
        if let Some(gemini) = canonical
            .extensions
            .get("gemini")
            .and_then(Value::as_object)
        {
            for (key, value) in gemini {
                if key == "raw_candidates" || object.contains_key(key) {
                    continue;
                }
                object.insert(key.clone(), value.clone());
            }
        }
    }
    Some(response)
}

fn canonical_to_gemini_response(
    canonical: &CanonicalResponse,
    report_context: &Value,
) -> Option<Value> {
    let outputs = if canonical.outputs.is_empty() {
        vec![CanonicalResponseOutput {
            index: 0,
            role: crate::protocol::canonical::CanonicalRole::Assistant,
            content: canonical.content.clone(),
            stop_reason: canonical.stop_reason.clone(),
            extensions: Default::default(),
        }]
    } else {
        canonical.outputs.clone()
    };
    let mut candidates = Vec::new();
    for output in outputs {
        let parts = canonical_blocks_to_gemini_parts(&output.content)?;
        let mut candidate = json!({
            "index": output.index,
            "content": {
                "role": "model",
                "parts": parts,
            },
            "finishReason": canonical_stop_reason_to_gemini(
                output.stop_reason.as_ref().or(canonical.stop_reason.as_ref())
            ),
        });
        if let Some(candidate_object) = candidate.as_object_mut() {
            if let Some(gemini) = output.extensions.get("gemini").and_then(Value::as_object) {
                if let Some(raw_finish_reason) = gemini.get("raw_finish_reason").cloned() {
                    candidate_object.insert("finishReason".to_string(), raw_finish_reason);
                }
                for (key, value) in gemini {
                    if key == "raw_finish_reason" {
                        continue;
                    }
                    candidate_object.entry(key.clone()).or_insert(value.clone());
                }
            }
        }
        candidates.push(candidate);
    }

    let mut response = Map::new();
    response.insert(
        "responseId".to_string(),
        Value::String(if canonical.id.trim().is_empty() {
            "resp-local-finalize".to_string()
        } else {
            canonical.id.clone()
        }),
    );
    response.insert(
        "modelVersion".to_string(),
        Value::String(
            if canonical.model.trim().is_empty() || canonical.model == "unknown" {
                report_context
                    .get("mapped_model")
                    .and_then(Value::as_str)
                    .or_else(|| report_context.get("model").and_then(Value::as_str))
                    .unwrap_or("unknown")
                    .to_string()
            } else {
                canonical.model.clone()
            },
        ),
    );
    response.insert("candidates".to_string(), Value::Array(candidates));
    if let Some(usage) = &canonical.usage {
        response.insert(
            "usageMetadata".to_string(),
            canonical_usage_to_gemini_usage_metadata(usage),
        );
    }
    Some(Value::Object(response))
}

fn canonical_blocks_to_gemini_parts(blocks: &[CanonicalContentBlock]) -> Option<Vec<Value>> {
    let mut parts = Vec::new();
    for block in blocks {
        if let Some(part) = canonical_block_to_gemini_part(block)? {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        parts.push(json!({ "text": "" }));
    }
    Some(parts)
}

fn canonical_block_to_gemini_part(block: &CanonicalContentBlock) -> Option<Option<Value>> {
    match block {
        CanonicalContentBlock::Text { text, .. } => Some(Some(json!({ "text": text }))),
        CanonicalContentBlock::Thinking {
            text, signature, ..
        } => {
            if text.trim().is_empty() {
                return Some(None);
            }
            let mut part = Map::new();
            part.insert("text".to_string(), Value::String(text.clone()));
            part.insert("thought".to_string(), Value::Bool(true));
            if let Some(signature) = signature.as_ref().filter(|value| !value.is_empty()) {
                part.insert(
                    "thoughtSignature".to_string(),
                    Value::String(signature.clone()),
                );
            }
            Some(Some(Value::Object(part)))
        }
        CanonicalContentBlock::ToolUse {
            id, name, input, ..
        } => Some(Some(json!({
            "functionCall": {
                "id": id,
                "name": name,
                "args": gemini_function_args(input),
            }
        }))),
        CanonicalContentBlock::ToolResult {
            tool_use_id,
            name,
            output,
            content_text,
            ..
        } => Some(Some(json!({
            "functionResponse": {
                "id": tool_use_id,
                "name": name.clone().unwrap_or_else(|| tool_use_id.clone()),
                "response": gemini_function_response(output.as_ref(), content_text.as_deref()),
            }
        }))),
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            ..
        } => Some(Some(canonical_media_to_gemini_part(
            media_type.as_deref().unwrap_or("image/png"),
            data.as_deref(),
            url.as_deref(),
        ))),
        CanonicalContentBlock::File {
            data,
            file_url,
            media_type,
            ..
        } => Some(Some(canonical_media_to_gemini_part(
            media_type.as_deref().unwrap_or("application/octet-stream"),
            data.as_deref(),
            file_url.as_deref(),
        ))),
        CanonicalContentBlock::Audio {
            data, media_type, ..
        } => Some(data.as_ref().map(|data| {
            json!({
                "inlineData": {
                    "mimeType": media_type.clone().unwrap_or_else(|| "audio/mpeg".to_string()),
                    "data": data,
                }
            })
        })),
        CanonicalContentBlock::Unknown { .. } => Some(None),
    }
}

fn canonical_media_to_gemini_part(
    media_type: &str,
    data: Option<&str>,
    url: Option<&str>,
) -> Value {
    if let Some(data) = data.filter(|value| !value.is_empty()) {
        return json!({
            "inlineData": {
                "mimeType": media_type,
                "data": data,
            }
        });
    }
    json!({
        "fileData": {
            "mimeType": media_type,
            "fileUri": url.unwrap_or_default(),
        }
    })
}

fn gemini_function_args(input: &Value) -> Value {
    match input {
        Value::Object(_) => input.clone(),
        Value::Null => json!({}),
        other => json!({ "value": other.clone() }),
    }
}

fn gemini_function_response(output: Option<&Value>, content_text: Option<&str>) -> Value {
    match output {
        Some(Value::Object(object)) => Value::Object(object.clone()),
        Some(value) => json!({ "result": value }),
        None => json!({ "result": content_text.unwrap_or_default() }),
    }
}

fn canonical_stop_reason_to_gemini(reason: Option<&CanonicalStopReason>) -> Value {
    Value::String(
        match reason {
            Some(CanonicalStopReason::MaxTokens) => "MAX_TOKENS",
            Some(CanonicalStopReason::ContentFiltered) | Some(CanonicalStopReason::Refusal) => {
                "SAFETY"
            }
            Some(CanonicalStopReason::Unknown) => "OTHER",
            _ => "STOP",
        }
        .to_string(),
    )
}

fn canonical_usage_to_gemini_usage_metadata(usage: &CanonicalUsage) -> Value {
    let input_tokens = canonical_usage_total_input_tokens(usage);
    let mut out = Map::new();
    out.insert("promptTokenCount".to_string(), Value::from(input_tokens));
    out.insert(
        "candidatesTokenCount".to_string(),
        Value::from(usage.output_tokens.saturating_sub(usage.reasoning_tokens)),
    );
    out.insert(
        "totalTokenCount".to_string(),
        Value::from(canonical_usage_total_tokens_for_inclusive_input(
            usage,
            input_tokens,
        )),
    );
    if usage.cache_read_tokens > 0 {
        out.insert(
            "cachedContentTokenCount".to_string(),
            Value::from(usage.cache_read_tokens),
        );
    }
    if usage.reasoning_tokens > 0 {
        out.insert(
            "thoughtsTokenCount".to_string(),
            Value::from(usage.reasoning_tokens),
        );
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CanonicalContentBlock;

    #[test]
    fn multipart_grounding_keeps_source_part_indices_and_target_coordinates() {
        let body = json!({"candidates": [{
            "content": {"parts": [
                {"text": "private", "thought": true}, null,
                {"text": "前"}, {"text": "中文"}
            ]}, "finishReason": "STOP",
            "groundingMetadata": {
                "groundingChunks": [{"web": {"uri": "https://example.com/source"}}],
                "groundingSupports": [{"segment": {"partIndex": 3, "startIndex": 0,
                    "endIndex": 6, "text": "中文"}, "groundingChunkIndices": [0]}]
            }
        }]});
        let canonical = from_raw(&body).expect("canonical response");
        let text_blocks = canonical.outputs[0]
            .content
            .iter()
            .filter_map(|block| match block {
                CanonicalContentBlock::Text { text, extensions } => Some((text, extensions)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(text_blocks.len(), 2);
        assert!(text_blocks[0].1.get("openai_responses").is_none());
        assert_eq!(
            text_blocks[1].1["openai_responses"]["annotations"][0]["start_index"],
            0
        );
        assert_eq!(
            text_blocks[1].1["openai_responses"]["annotations"][0]["end_index"],
            2
        );
        assert_eq!(
            text_blocks[1].1["claude"]["citations"][0]["cited_text"],
            "中文"
        );
        let chat = crate::formats::openai::chat::response::to_raw(&canonical);
        assert_eq!(chat["choices"][0]["message"]["content"], "前中文");
        assert_eq!(
            chat["choices"][0]["message"]["annotations"][0]["start_index"],
            1
        );
        assert_eq!(
            chat["choices"][0]["message"]["annotations"][0]["end_index"],
            3
        );
        let responses =
            crate::formats::openai::responses::response::to_raw(&canonical, &json!({}), false);
        let message = responses["output"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "message")
            .expect("message");
        assert_eq!(message["content"][0]["text"], "前中文");
        assert_eq!(message["content"][0]["annotations"][0]["start_index"], 1);
        assert_eq!(message["content"][0]["annotations"][0]["end_index"], 3);
    }

    #[test]
    fn grounding_drops_unmatchable_spans_without_unanchored_fallback() {
        for segment in [
            json!({"endIndex": 6}),                 // default part 0 is hidden reasoning
            json!({"partIndex": 1, "endIndex": 6}), // unsupported part
            json!({"partIndex": 2, "endIndex": 6}), // image part
            json!({"partIndex": 3, "startIndex": 1, "endIndex": 6}), // UTF-8 interior
            json!({"partIndex": 3, "endIndex": 7}), // stale extent
            json!({"partIndex": 3, "endIndex": 6, "text": "别的"}),
            json!({"partIndex": 99, "endIndex": 6}),
        ] {
            let body = json!({"candidates": [{"content": {"parts": [
                {"text": "中文", "thought": true}, null,
                {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}},
                {"text": "中文"}
            ]}, "groundingMetadata": {
                "groundingChunks": [{"web": {"uri": "https://example.com/source"}}],
                "groundingSupports": [{"segment": segment, "groundingChunkIndices": [0]}]
            }}]});
            let canonical = from_raw(&body).expect("response remains valid");
            for block in &canonical.outputs[0].content {
                if let CanonicalContentBlock::Text { extensions, .. } = block {
                    assert!(extensions.get("openai_responses").is_none(), "{segment}");
                    assert!(extensions.get("claude").is_none(), "{segment}");
                }
            }
        }
    }
    /// Gemini omits `groundingSupports` when it cannot anchor the answer to a
    /// span. The sources are still real, so they must survive unanchored
    /// rather than be dropped for lacking offsets.
    #[test]
    fn grounding_without_supports_still_yields_unanchored_citations() {
        let body = json!({
            "responseId": "resp-unanchored",
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "Rust 1.95 is current."}]},
                "finishReason": "STOP",
                "groundingMetadata": {
                    "groundingChunks": [
                        {"web": {"uri": "https://blog.rust-lang.org/", "title": "Rust Blog"}},
                        {"web": {"title": "no uri here"}}
                    ]
                }
            }]
        });

        let canonical = from_raw(&body).expect("canonical");
        let CanonicalContentBlock::Text { extensions, .. } = &canonical.outputs[0].content[0]
        else {
            panic!("expected a text block");
        };

        assert_eq!(
            extensions["claude"]["citations"],
            json!([{
                "type": "web_search_result_location",
                "url": "https://blog.rust-lang.org/",
                "title": "Rust Blog",
            }])
        );
        assert_eq!(
            extensions["openai_responses"]["annotations"],
            json!([{
                "type": "url_citation",
                "url": "https://blog.rust-lang.org/",
                "title": "Rust Blog",
            }])
        );
    }

    #[test]
    fn gemini_response_without_visible_parts_is_not_success() {
        let body = json!({
            "candidates": [{
                "content": {"role": "model"},
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": {
                "promptTokenCount": 8,
                "candidatesTokenCount": 1,
                "thoughtsTokenCount": 25,
                "totalTokenCount": 34
            },
            "modelVersion": "gemini-3-flash-preview",
            "responseId": "resp-empty"
        });

        assert!(from_raw(&body).is_none());
    }

    #[test]
    fn gemini_response_with_only_thought_parts_is_success() {
        let body = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "hidden plan", "thought": true}]
                },
                "finishReason": "MAX_TOKENS"
            }],
            "modelVersion": "gemini-3-flash-preview",
            "responseId": "resp-thought-only"
        });

        let canonical = from_raw(&body).expect("thought text is representable output");
        assert!(matches!(
            canonical.content.first(),
            Some(CanonicalContentBlock::Thinking { text, .. }) if text == "hidden plan"
        ));
        assert!(matches!(
            canonical.stop_reason,
            Some(CanonicalStopReason::MaxTokens)
        ));

        let openai = crate::canonical_to_openai_chat_response(&canonical);
        assert_eq!(
            openai["choices"][0]["message"]["reasoning_content"],
            "hidden plan"
        );
        assert_eq!(openai["choices"][0]["finish_reason"], "length");
    }

    #[test]
    fn gemini_response_with_signature_only_reasoning_exhaustion_is_success() {
        let body = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "text": "",
                        "thoughtSignature": "opaque-thought-signature"
                    }]
                },
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": {
                "promptTokenCount": 22,
                "thoughtsTokenCount": 29,
                "totalTokenCount": 51
            },
            "modelVersion": "gemini-3.7-flash-tiered",
            "responseId": "resp-signature-only"
        });

        let canonical = from_raw(&body).expect("reasoning exhaustion is a valid terminal");
        assert!(matches!(
            canonical.content.first(),
            Some(CanonicalContentBlock::Thinking {
                text,
                signature: Some(signature),
                ..
            }) if text.is_empty() && signature == "opaque-thought-signature"
        ));
        assert!(matches!(
            canonical.stop_reason,
            Some(CanonicalStopReason::MaxTokens)
        ));
        assert_eq!(
            canonical.usage.as_ref().map(|usage| usage.reasoning_tokens),
            Some(29)
        );

        let openai = crate::canonical_to_openai_chat_response(&canonical);
        assert_eq!(openai["choices"][0]["finish_reason"], "length");
        assert_eq!(
            openai["usage"]["completion_tokens_details"]["reasoning_tokens"],
            29
        );
    }

    #[test]
    fn gemini_signature_only_terminal_without_reasoning_usage_is_not_success() {
        let body = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "text": "",
                        "thoughtSignature": "opaque-thought-signature"
                    }]
                },
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": {
                "promptTokenCount": 22,
                "thoughtsTokenCount": 0,
                "totalTokenCount": 22
            }
        });

        assert!(from_raw(&body).is_none());
    }

    #[test]
    fn gemini_response_with_function_call_is_visible_output() {
        let body = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "lookup",
                            "args": {"query": "weather"}
                        }
                    }]
                },
                "finishReason": "STOP"
            }],
            "modelVersion": "gemini-3-flash-preview",
            "responseId": "resp-tool"
        });

        let canonical = from_raw(&body).expect("function call should be visible output");
        assert!(matches!(
            canonical.content.first(),
            Some(CanonicalContentBlock::ToolUse { name, .. }) if name == "lookup"
        ));
    }
}
