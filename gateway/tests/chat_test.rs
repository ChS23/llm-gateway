mod common;

use serde_json::json;

#[tokio::test]
async fn test_chat_completions_json() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;

    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .authorization_bearer(&key)
        .await;

    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["model"], "test-model");
    assert!(body["choices"][0]["message"]["content"].is_string());
    assert!(body["usage"]["total_tokens"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn test_chat_completions_with_temperature() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;

    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hi"}],
            "temperature": 0.7,
            "max_tokens": 100
        }))
        .authorization_bearer(&key)
        .await;

    resp.assert_status_ok();
}

#[tokio::test]
async fn test_responses_api() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;

    let resp = server
        .post("/v1/responses")
        .json(&json!({
            "model": "test-model",
            "input": "What is Rust?"
        }))
        .authorization_bearer(&key)
        .await;

    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["object"], "response");
    assert!(body["output_text"].is_string());
    assert!(body["output"].as_array().unwrap().len() > 0);
}

#[tokio::test]
async fn test_responses_api_with_messages() {
    let server = require_server!();
    let key = common::create_key(&server, &["chat"]).await;

    let resp = server
        .post("/v1/responses")
        .json(&json!({
            "model": "test-model",
            "input": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hi"}
            ]
        }))
        .authorization_bearer(&key)
        .await;

    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["object"], "response");
}
