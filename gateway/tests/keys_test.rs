mod common;

use serde_json::json;

#[tokio::test]
async fn test_api_key_lifecycle() {
    let server = require_server!();
    let admin = "test-admin-key";

    // Create
    let resp = server
        .post("/admin/keys")
        .json(&json!({"name": "lifecycle-test", "scopes": ["chat"]}))
        .authorization_bearer(admin)
        .await;
    assert_eq!(resp.status_code().as_u16(), 201);
    let body: serde_json::Value = resp.json();
    let raw_key = body["key"].as_str().unwrap().to_string();
    assert!(raw_key.starts_with("sk-gw-"));

    // List
    let resp = server.get("/admin/keys").authorization_bearer(admin).await;
    resp.assert_status_ok();
    let list: Vec<serde_json::Value> = resp.json();
    assert!(list.iter().any(|k| k["name"] == "lifecycle-test"));

    // Use key — should not be 401
    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({"model":"test-model","messages":[{"role":"user","content":"hi"}]}))
        .authorization_bearer(&raw_key)
        .await;
    assert_ne!(resp.status_code().as_u16(), 401);

    // Delete
    let id = list.iter().find(|k| k["name"] == "lifecycle-test").unwrap()["id"]
        .as_str()
        .unwrap();
    let resp = server
        .delete(&format!("/admin/keys/{id}"))
        .authorization_bearer(admin)
        .await;
    resp.assert_status_no_content();

    // Use deleted key — should be 401
    let resp = server
        .post("/v1/chat/completions")
        .json(&json!({"model":"test-model","messages":[{"role":"user","content":"hi"}]}))
        .authorization_bearer(&raw_key)
        .await;
    resp.assert_status_unauthorized();
}
