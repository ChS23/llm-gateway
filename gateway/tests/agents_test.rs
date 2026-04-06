mod common;

use serde_json::json;

#[tokio::test]
async fn test_agent_crud_and_discovery() {
    let server = require_server!();
    let admin = "test-admin-key";
    let name = format!("agent-{}", uuid::Uuid::new_v4());

    // Create
    let resp = server
        .post("/admin/agents")
        .json(&json!({
            "name": name,
            "description": "Test agent",
            "url": "https://agent.test",
            "skills": [{"id": "s1", "name": "Skill One"}],
            "capabilities": {"streaming": true}
        }))
        .authorization_bearer(admin)
        .await;
    assert_eq!(resp.status_code().as_u16(), 201);
    let created: serde_json::Value = resp.json();
    let id = created["id"].as_str().unwrap();

    // List
    let resp = server
        .get("/admin/agents")
        .authorization_bearer(admin)
        .await;
    resp.assert_status_ok();
    let list: Vec<serde_json::Value> = resp.json();
    assert!(list.iter().any(|a| a["name"] == name));

    // A2A Discovery
    let resp = server
        .get(&format!("/admin/agents/{id}/.well-known/agent-card.json"))
        .authorization_bearer(admin)
        .await;
    resp.assert_status_ok();
    let card: serde_json::Value = resp.json();
    assert_eq!(card["name"], name);

    // Update
    let resp = server
        .put(&format!("/admin/agents/{id}"))
        .json(&json!({"version": "2.0.0"}))
        .authorization_bearer(admin)
        .await;
    resp.assert_status_ok();
    let updated: serde_json::Value = resp.json();
    assert_eq!(updated["version"], "2.0.0");

    // Delete
    let resp = server
        .delete(&format!("/admin/agents/{id}"))
        .authorization_bearer(admin)
        .await;
    resp.assert_status_no_content();
}

#[tokio::test]
async fn test_agent_no_skills_returns_400() {
    let server = require_server!();
    let resp = server
        .post("/admin/agents")
        .json(&json!({
            "name": "no-skills",
            "description": "x",
            "url": "http://x",
            "skills": []
        }))
        .authorization_bearer("test-admin-key")
        .await;
    resp.assert_status_bad_request();
}
