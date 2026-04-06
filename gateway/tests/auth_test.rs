mod common;

use serde_json::json;

#[tokio::test]
async fn test_no_auth_returns_401() {
    let server = require_server!();
    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({"model":"x","messages":[{"role":"user","content":"hi"}]}))
        .await;
    resp.assert_status_unauthorized();
}

#[tokio::test]
async fn test_invalid_key_returns_401() {
    let server = require_server!();
    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({"model":"x","messages":[{"role":"user","content":"hi"}]}))
        .authorization_bearer("sk-gw-invalid-key")
        .await;
    resp.assert_status_unauthorized();
}

#[tokio::test]
async fn test_valid_key_not_401() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;
    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({"model":"test-model","messages":[{"role":"user","content":"hi"}]}))
        .authorization_bearer(&key)
        .await;
    // Might be 502 (mock not running) but NOT 401
    assert_ne!(resp.status_code(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_chat_scope_on_admin_returns_403() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;
    let resp = server
        .get("/admin/providers")
        .authorization_bearer(&key)
        .await;
    resp.assert_status_forbidden();
}
