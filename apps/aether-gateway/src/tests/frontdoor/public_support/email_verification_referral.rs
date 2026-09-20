use super::{
    build_router_with_state, json, sample_auth_user, start_server, AppState, StatusCode, Utc,
    TEST_EMAIL_VERIFICATION_TOKEN,
};

async fn verify_email(gateway_url: &str, email: &str, code: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{gateway_url}/api/auth/verify-email"))
        .json(&json!({
            "email": email,
            "code": code,
            "verification_token": TEST_EMAIL_VERIFICATION_TOKEN,
        }))
        .send()
        .await
        .expect("email verification request should succeed")
}

#[tokio::test]
async fn gateway_verifies_existing_local_email_once_after_consuming_challenge() {
    let now = Utc::now();
    let mut user = sample_auth_user(now);
    user.email_verified = false;
    let user_id = user.id.clone();
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([user])
        .with_auth_email_verification_pending_for_tests(
            "alice@example.com",
            "123456",
            TEST_EMAIL_VERIFICATION_TOKEN,
            now,
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let verified = verify_email(&gateway_url, "alice@example.com", "123456").await;
    assert_eq!(verified.status(), StatusCode::OK);
    assert!(
        state
            .find_user_auth_by_id(&user_id)
            .await
            .expect("user lookup should succeed")
            .expect("existing user should remain")
            .email_verified
    );

    let repeated = verify_email(&gateway_url, "alice@example.com", "123456").await;
    assert_eq!(repeated.status(), StatusCode::BAD_REQUEST);
    assert!(
        state
            .find_user_auth_by_id(&user_id)
            .await
            .expect("user lookup should succeed")
            .expect("existing user should remain")
            .email_verified
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_email_verification_does_not_mutate_username_collision() {
    let now = Utc::now();
    let mut collision = sample_auth_user(now);
    collision.id = "user-auth-collision".to_string();
    collision.email = Some("other@example.com".to_string());
    collision.username = "alice@example.com".to_string();
    collision.email_verified = false;
    let collision_id = collision.id.clone();
    let state = AppState::new()
        .expect("gateway should build")
        .with_auth_users_for_tests([collision])
        .with_auth_email_verification_pending_for_tests(
            "alice@example.com",
            "123456",
            TEST_EMAIL_VERIFICATION_TOKEN,
            now,
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let verified = verify_email(&gateway_url, "alice@example.com", "123456").await;
    assert_eq!(verified.status(), StatusCode::OK);
    assert!(
        !state
            .find_user_auth_by_id(&collision_id)
            .await
            .expect("collision user lookup should succeed")
            .expect("collision user should remain")
            .email_verified
    );

    gateway_handle.abort();
}
