mod common;

use serde_json::json;

#[tokio::test]
async fn test_provider_crud() {
    let server = require_server!();
    let admin = "test-admin-key";
    let name = format!("prov-{}", uuid::Uuid::new_v4());

    // Create
    let resp = server
        .post("/admin/providers")
        .json(&json!({
            "name": name,
            "provider_type": "mock",
            "base_url": "http://localhost:9999",
            "models": ["model-a"],
            "weight": 2
        }))
        .authorization_bearer(admin)
        .await;
    assert_eq!(resp.status_code().as_u16(), 201);
    let created: serde_json::Value = resp.json();
    let id = created["id"].as_str().unwrap();

    // List
    let resp = server
        .get("/admin/providers")
        .authorization_bearer(admin)
        .await;
    resp.assert_status_ok();
    let list: Vec<serde_json::Value> = resp.json();
    assert!(list.iter().any(|p| p["name"] == name));

    // Get
    let resp = server
        .get(&format!("/admin/providers/{id}"))
        .authorization_bearer(admin)
        .await;
    resp.assert_status_ok();
    let got: serde_json::Value = resp.json();
    assert_eq!(got["name"], name);

    // Update
    let resp = server
        .put(&format!("/admin/providers/{id}"))
        .json(&json!({"weight": 5}))
        .authorization_bearer(admin)
        .await;
    resp.assert_status_ok();
    let updated: serde_json::Value = resp.json();
    assert_eq!(updated["weight"], 5);

    // Delete
    let resp = server
        .delete(&format!("/admin/providers/{id}"))
        .authorization_bearer(admin)
        .await;
    resp.assert_status_no_content();

    // List after delete — should not contain
    let resp = server
        .get("/admin/providers")
        .authorization_bearer(admin)
        .await;
    let list: Vec<serde_json::Value> = resp.json();
    assert!(!list.iter().any(|p| p["name"] == name));
}
