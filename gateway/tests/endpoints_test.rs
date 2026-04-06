mod common;

use serde_json::json;

#[tokio::test]
async fn test_list_models() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;
    let resp = server.get("/v1/models").authorization_bearer(&key).await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["object"], "list");
    assert!(body["data"].as_array().unwrap().len() > 0);
}

#[tokio::test]
async fn test_embeddings_returns_501() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;
    let resp = server
        .post("/v1/embeddings")
        .json(&json!({"model": "text-embedding", "input": "test"}))
        .authorization_bearer(&key)
        .await;
    assert_eq!(resp.status_code().as_u16(), 501);
}

#[tokio::test]
async fn test_unknown_model_returns_400() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;
    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({
            "model": "nonexistent-model-xyz",
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .authorization_bearer(&key)
        .await;
    resp.assert_status_bad_request();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["type"], "invalid_model");
}

#[tokio::test]
async fn test_openapi_json() {
    let server = require_server!();
    let resp = server.get("/openapi.json").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body["openapi"].is_string());
}
