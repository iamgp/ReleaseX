use mockito::{Matcher, Server};
use relx::github::{GitHubClient, RepoRef};

fn test_client(server: &Server) -> GitHubClient {
    GitHubClient::new(
        &server.url(),
        "test-token",
        RepoRef {
            owner: "acme".into(),
            name: "demo".into(),
        },
    )
    .expect("client")
}

#[test]
fn create_pr_sends_correct_body() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/repos/acme/demo/pulls")
        .match_header("Authorization", "Bearer test-token")
        .match_header("Accept", "application/vnd.github+json")
        .match_body(Matcher::Json(serde_json::json!({
            "title": "chore(release): v1.0.0",
            "head": "relx/release/v1.0.0",
            "base": "main",
            "body": "Release notes"
        })))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"number": 42}"#)
        .create();

    let client = test_client(&server);
    let pr = client
        .create_pr(
            "chore(release): v1.0.0",
            "relx/release/v1.0.0",
            "main",
            "Release notes",
        )
        .expect("create_pr should succeed");

    assert_eq!(pr.number, 42);
    mock.assert();
}

#[test]
fn update_pr_sends_patch_request() {
    let mut server = Server::new();
    let mock = server
        .mock("PATCH", "/repos/acme/demo/pulls/7")
        .match_header("Authorization", "Bearer test-token")
        .match_body(Matcher::Json(serde_json::json!({
            "title": "chore(release): v2.0.0",
            "body": "Updated notes"
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"number": 7}"#)
        .create();

    let client = test_client(&server);
    let pr = client
        .update_pr(7, "chore(release): v2.0.0", "Updated notes")
        .expect("update_pr should succeed");

    assert_eq!(pr.number, 7);
    mock.assert();
}

#[test]
fn find_open_pr_returns_first_match() {
    let mut server = Server::new();
    let mock = server
        .mock("GET", "/repos/acme/demo/pulls")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "open".into()),
            Matcher::UrlEncoded("head".into(), "acme:relx/release/v1.0.0".into()),
            Matcher::UrlEncoded("base".into(), "main".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"number": 10}]"#)
        .create();

    let client = test_client(&server);
    let pr = client
        .find_open_pr("relx/release/v1.0.0", "main")
        .expect("find_open_pr should succeed");

    assert_eq!(pr.unwrap().number, 10);
    mock.assert();
}

#[test]
fn find_open_pr_returns_none_when_empty() {
    let mut server = Server::new();
    let mock = server
        .mock("GET", "/repos/acme/demo/pulls")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("state".into(), "open".into()),
            Matcher::UrlEncoded("head".into(), "acme:feature".into()),
            Matcher::UrlEncoded("base".into(), "main".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();

    let client = test_client(&server);
    let pr = client
        .find_open_pr("feature", "main")
        .expect("find_open_pr should succeed");

    assert!(pr.is_none());
    mock.assert();
}

#[test]
fn create_release_sends_correct_body() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/repos/acme/demo/releases")
        .match_header("Authorization", "Bearer test-token")
        .match_body(Matcher::Json(serde_json::json!({
            "tag_name": "v1.0.0",
            "target_commitish": "main",
            "name": "Release v1.0.0",
            "body": "## What's new\n\n- feature",
            "generate_release_notes": false
        })))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id": 100}"#)
        .create();

    let client = test_client(&server);
    let release = client
        .create_release(
            "v1.0.0",
            "Release v1.0.0",
            "## What's new\n\n- feature",
            "main",
        )
        .expect("create_release should succeed");

    assert_eq!(release.id, 100);
    mock.assert();
}

#[test]
fn update_release_sends_patch_request() {
    let mut server = Server::new();
    let mock = server
        .mock("PATCH", "/repos/acme/demo/releases/100")
        .match_body(Matcher::Json(serde_json::json!({
            "name": "Release v1.0.1",
            "body": "Updated notes"
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id": 100}"#)
        .create();

    let client = test_client(&server);
    let release = client
        .update_release(100, "Release v1.0.1", "Updated notes")
        .expect("update_release should succeed");

    assert_eq!(release.id, 100);
    mock.assert();
}

#[test]
fn find_release_by_tag_returns_release() {
    let mut server = Server::new();
    let mock = server
        .mock("GET", "/repos/acme/demo/releases/tags/v1.0.0")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id": 55}"#)
        .create();

    let client = test_client(&server);
    let release = client
        .find_release_by_tag("v1.0.0")
        .expect("find_release_by_tag should succeed");

    assert_eq!(release.unwrap().id, 55);
    mock.assert();
}

#[test]
fn find_release_by_tag_returns_none_on_404() {
    let mut server = Server::new();
    let mock = server
        .mock("GET", "/repos/acme/demo/releases/tags/v99.0.0")
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_body(r#"{"message": "Not Found"}"#)
        .create();

    let client = test_client(&server);
    let release = client
        .find_release_by_tag("v99.0.0")
        .expect("find_release_by_tag should succeed for 404");

    assert!(release.is_none());
    mock.assert();
}

#[test]
fn add_labels_sends_correct_body() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/repos/acme/demo/issues/42/labels")
        .match_header("Authorization", "Bearer test-token")
        .match_body(Matcher::Json(serde_json::json!({
            "labels": ["autorelease: pending", "release"]
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();

    let client = test_client(&server);
    client
        .add_labels(
            42,
            &["autorelease: pending".to_string(), "release".to_string()],
        )
        .expect("add_labels should succeed");

    mock.assert();
}

#[test]
fn ensure_label_creates_label_when_not_found() {
    let mut server = Server::new();
    let get_mock = server
        .mock("GET", "/repos/acme/demo/labels/autorelease:%20pending")
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_body(r#"{"message": "Not Found"}"#)
        .create();
    let post_mock = server
        .mock("POST", "/repos/acme/demo/labels")
        .match_body(Matcher::Json(serde_json::json!({
            "name": "autorelease: pending",
            "color": "ededed",
            "description": "Managed by relx"
        })))
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id": 1, "name": "autorelease: pending"}"#)
        .create();

    let client = test_client(&server);
    client
        .ensure_label("autorelease: pending")
        .expect("ensure_label should succeed");

    get_mock.assert();
    post_mock.assert();
}

#[test]
fn ensure_label_skips_creation_when_exists() {
    let mut server = Server::new();
    let get_mock = server
        .mock("GET", "/repos/acme/demo/labels/release")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id": 1, "name": "release"}"#)
        .create();
    let post_mock = server
        .mock("POST", "/repos/acme/demo/labels")
        .expect(0)
        .create();

    let client = test_client(&server);
    client
        .ensure_label("release")
        .expect("ensure_label should succeed");

    get_mock.assert();
    post_mock.assert();
}

#[test]
fn create_pr_returns_error_on_422() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/repos/acme/demo/pulls")
        .with_status(422)
        .with_header("content-type", "application/json")
        .with_body(r#"{"message": "Validation Failed"}"#)
        .create();

    let client = test_client(&server);
    let err = client
        .create_pr("title", "head", "base", "body")
        .expect_err("should fail on 422");

    assert!(
        err.to_string().contains("422"),
        "error should mention status code: {err}"
    );
    mock.assert();
}

#[test]
fn create_release_returns_error_on_404() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/repos/acme/demo/releases")
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_body(r#"{"message": "Not Found"}"#)
        .create();

    let client = test_client(&server);
    let err = client
        .create_release("v1.0.0", "Release v1.0.0", "notes", "main")
        .expect_err("should fail on 404");

    assert!(
        err.to_string().contains("404"),
        "error should mention status code: {err}"
    );
    mock.assert();
}

#[test]
fn add_labels_returns_error_on_404() {
    let mut server = Server::new();
    let mock = server
        .mock("POST", "/repos/acme/demo/issues/999/labels")
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_body(r#"{"message": "Not Found"}"#)
        .create();

    let client = test_client(&server);
    let err = client
        .add_labels(999, &["bug".to_string()])
        .expect_err("should fail on 404");

    assert!(
        err.to_string().contains("404"),
        "error should mention status code: {err}"
    );
    mock.assert();
}
