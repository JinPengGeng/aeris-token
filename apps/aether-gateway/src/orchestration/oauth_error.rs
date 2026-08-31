pub(crate) fn oauth_status_may_be_invalid(status_code: u16, response_text: Option<&str>) -> bool {
    if status_code == 401 {
        return true;
    }
    if status_code != 403 {
        return false;
    }

    response_text.is_some_and(|response_text| {
        oauth_response_proves_access_token_invalid(response_text)
            || (serde_json::from_str::<serde_json::Value>(response_text).is_err()
                && response_has_oauth_invalid_phrase(response_text))
    })
}

pub(crate) fn oauth_status_proves_access_token_invalid(
    status_code: u16,
    response_text: Option<&str>,
) -> bool {
    if status_code == 401 {
        return true;
    }
    if status_code != 403 {
        return false;
    }

    response_text.is_some_and(oauth_response_proves_access_token_invalid)
}

pub(crate) fn oauth_response_proves_access_token_invalid(response_text: &str) -> bool {
    if let Ok(body) = serde_json::from_str::<serde_json::Value>(response_text) {
        let taxonomy_values = ["/error/type", "/error/code", "/type", "/code"]
            .into_iter()
            .filter_map(|path| body.pointer(path))
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("error"))
            .collect::<Vec<_>>();
        if !taxonomy_values.is_empty() {
            return taxonomy_values
                .into_iter()
                .any(is_oauth_invalid_error_taxonomy);
        }

        if let Some(error) = body.get("error").and_then(serde_json::Value::as_str) {
            if is_oauth_invalid_error_taxonomy(error) {
                return true;
            }
        }

        return false;
    }

    false
}

pub(crate) fn is_oauth_invalid_error_taxonomy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "authentication_error"
            | "invalid_authentication_token"
            | "invalid_token"
            | "oauth_token_invalid"
            | "token_invalid"
            | "token_expired"
            | "unauthenticated"
            | "biscuit_baker_service_auth_credential_error_status"
    )
}

fn response_has_oauth_invalid_phrase(response_text: &str) -> bool {
    let response_text = response_text.to_ascii_lowercase();
    if [
        "oauth_token_invalid",
        "invalid_token",
        "biscuit_baker_service_auth_credential_error_status",
    ]
    .iter()
    .any(|taxonomy| contains_ascii_taxonomy_token(&response_text, taxonomy))
    {
        return true;
    }

    [
        "oauth token is invalid",
        "oauth token is expired",
        "oauth token has expired",
        "invalid access token",
        "access token invalid",
        "access token expired",
        "expired access token",
        "authentication token has been invalidated",
        "token has been invalidated",
        "personal access token owner is inactive",
        "security token included in the request is expired",
    ]
    .iter()
    .any(|needle| response_text.contains(needle))
}

fn contains_ascii_taxonomy_token(text: &str, taxonomy: &str) -> bool {
    text.match_indices(taxonomy).any(|(start, matched)| {
        let end = start + matched.len();
        let is_identifier_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
        let has_left_boundary = start == 0 || !is_identifier_byte(text.as_bytes()[start - 1]);
        let has_right_boundary = end == text.len() || !is_identifier_byte(text.as_bytes()[end]);
        has_left_boundary && has_right_boundary
    })
}

#[cfg(test)]
mod tests {
    use super::{
        oauth_response_proves_access_token_invalid, oauth_status_may_be_invalid,
        oauth_status_proves_access_token_invalid,
    };

    #[test]
    fn trusted_taxonomy_checks_every_type_and_code_field() {
        for response_text in [
            r#"{"error":{"type":"invalid_request_error","code":"invalid_token"}}"#,
            r#"{"error":{"type":"authentication_error","code":"permission_denied"}}"#,
            r#"{"type":"invalid_request_error","code":"oauth_token_invalid"}"#,
            r#"{"type":"authentication_error","code":"permission_denied"}"#,
        ] {
            assert!(oauth_response_proves_access_token_invalid(response_text));
            assert!(oauth_status_may_be_invalid(403, Some(response_text)));
            assert!(oauth_status_proves_access_token_invalid(
                403,
                Some(response_text)
            ));
        }
    }

    #[test]
    fn untrusted_structured_taxonomy_does_not_use_message_phrases_as_proof() {
        let response_text = r#"{"error":{"type":"invalid_request_error","code":"permission_denied","message":"authentication token has been invalidated"}}"#;
        assert!(!oauth_response_proves_access_token_invalid(response_text));
        assert!(!oauth_status_may_be_invalid(403, Some(response_text)));
        assert!(!oauth_status_proves_access_token_invalid(
            403,
            Some(response_text)
        ));
    }

    #[test]
    fn plain_text_phrase_is_only_a_legacy_oauth_candidate_heuristic() {
        let response_text = "oauth token is expired";
        assert!(!oauth_response_proves_access_token_invalid(response_text));
        assert!(oauth_status_may_be_invalid(403, Some(response_text)));
        assert!(!oauth_status_proves_access_token_invalid(
            403,
            Some(response_text)
        ));
    }
}
