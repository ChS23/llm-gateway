mod common;

#[tokio::test]
async fn test_health_returns_200() {
    let server = require_server!();
    let resp = server.get("/health").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["postgres"], "ok");
}

#[tokio::test]
async fn test_health_providers() {
    let server = require_server!();
    let resp = server.get("/health/providers").await;
    resp.assert_status_ok();
}
