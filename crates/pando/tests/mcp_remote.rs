//! The remote MCP endpoint (ADR-026): an agent with a URL and a token reads, writes, opens and
//! lands change requests without a checkout, limited to what its token allows.

use pando::remote::GrpcRemote;
use pando_proto::{CreateRepositoryRequest, Protection, RepositoryRef, Role, SetMemberRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod common;

/// One MCP tool call over HTTP; returns (is_error, text).
async fn call(addr: &str, token: &str, tool: &str, args: Value) -> (bool, String) {
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {}
            },
            "name": tool, "arguments": args
        }
    })
    .to_string();
    let auth = if token.is_empty() { String::new() } else { format!("authorization: Bearer {token}\r\n") };
    let req = format!(
        "POST /mcp HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-type: application/json\r\naccept: application/json, text/event-stream\r\n\
         mcp-protocol-version: 2026-07-28\r\nmcp-method: tools/call\r\nmcp-name: {tool}\r\n{auth}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let json: Value = serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let result = &json["result"];
    let text = result["content"]
        .as_array()
        .map(|c| c.iter().filter_map(|x| x["text"].as_str()).collect::<String>())
        .unwrap_or_default();
    (result["isError"].as_bool().unwrap_or(true), text)
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_work_through_mcp_without_a_checkout() {
    let s = common::start().await;
    let admin = GrpcRemote::connect(&s.url, None, Some("admin-token")).unwrap();
    admin
        .platform()
        .create_repository(CreateRepositoryRequest {
            owner: "lumen".into(),
            name: "fleet".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    for who in ["agent/fix-01", "mira"] {
        let m = SetMemberRequest { org: "lumen".into(), login: who.into(), role: Role::Write as i32 };
        admin.platform().set_member(m).await.unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    tokio::spawn(pando::mcp_remote::RemoteMcp::new(s.url.clone()).serve_http(addr, vec![]));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let addr = addr.to_string();
    let agent = "agent-fix-01-token";

    // The first change in an empty repository, then reading it back.
    let (e, t) = call(
        &addr,
        agent,
        "pando_write",
        json!({"repo": "lumen/fleet", "branch": "main",
        "files": [{"path": "billing/tax.rs", "content": "rate = 19\n"}], "message": "Start billing"}),
    )
    .await;
    assert!(!e && t.starts_with("landed 1 on main"), "{t}");
    assert_eq!(
        call(&addr, agent, "pando_read_file", json!({"repo": "lumen/fleet", "path": "billing/tax.rs"})).await,
        (false, "rate = 19\n".into())
    );
    let (_, history) = call(&addr, agent, "pando_history", json!({"repo": "lumen/fleet"})).await;
    assert!(history.contains("agent/fix-01 | Start billing"), "the server records who made it: {history}");

    // On a protected branch the same call opens a change request instead.
    admin
        .reviews()
        .set_protection(Protection {
            repository: Some(RepositoryRef { owner: "lumen".into(), name: "fleet".into() }),
            pattern: "main".into(),
            required_approvals: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    let (e, t) = call(
        &addr,
        agent,
        "pando_write",
        json!({"repo": "lumen/fleet", "branch": "main",
        "files": [{"path": "billing/tax.rs", "content": "rate = 20\n"}], "message": "Raise the tax rate to 20"}),
    )
    .await;
    assert!(!e && t.contains("went for review") && t.contains("Needs 1 more approval"), "{t}");
    let (_, inbox) = call(&addr, "mira-token", "pando_inbox", json!({})).await;
    assert!(inbox.contains("Needs a reviewer") && inbox.contains("#1"), "{inbox}");
    let (e, t) =
        call(&addr, "mira-token", "pando_review", json!({"repo": "lumen/fleet", "number": 1, "verdict": "approve"}))
            .await;
    assert!(!e && t.contains("ready to land"), "{t}");
    let (e, t) = call(&addr, agent, "pando_land_change_request", json!({"repo": "lumen/fleet", "number": 1})).await;
    assert!(!e, "{t}");
    assert_eq!(
        call(&addr, agent, "pando_read_file", json!({"repo": "lumen/fleet", "path": "billing/tax.rs"})).await.1,
        "rate = 20\n"
    );

    // No token, or a token that can't see the repository: nothing.
    let (e, t) = call(&addr, "", "pando_repositories", json!({})).await;
    assert!(e && t.contains("Authorization: Bearer"), "{t}");
    let (e, t) =
        call(&addr, "eve-token", "pando_read_file", json!({"repo": "lumen/fleet", "path": "billing/tax.rs"})).await;
    assert!(e && t.contains("no repository `lumen/fleet`"), "{t}");
}
