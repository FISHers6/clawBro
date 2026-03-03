//! E2E tests: Feishu/Lark channel authentication + WebSocket connection
//!
//! Tests are ignored by default; opt in with --ignored flag.
//!
//! To run:
//!   LARK_APP_ID=cli_YOUR_APP_ID \
//!   LARK_APP_SECRET=YOUR_APP_SECRET \
//!   cargo test -p qai-server --test e2e_lark -- --ignored --nocapture
//!
//! For send test, also set:
//!   LARK_TEST_OPEN_ID=ou_xxx  (open_id of the user to DM)

const FEISHU_BASE: &str = "https://open.feishu.cn/open-apis";
/// Feishu WS endpoint discovery URL (official Go SDK: GenEndpointUri = "/callback/ws/endpoint")
const FEISHU_WS_ENDPOINT_URL: &str = "https://open.feishu.cn/callback/ws/endpoint";

/// Get Feishu app_access_token via REST API.
async fn get_feishu_token(app_id: &str, app_secret: &str) -> String {
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(format!("{FEISHU_BASE}/auth/v3/app_access_token/internal"))
        .json(&serde_json::json!({"app_id": app_id, "app_secret": app_secret}))
        .send()
        .await
        .expect("HTTP request failed")
        .json()
        .await
        .expect("JSON parse failed");
    assert_eq!(
        resp["code"].as_i64().unwrap_or(-1),
        0,
        "Feishu auth failed: {resp:?}"
    );
    resp["app_access_token"]
        .as_str()
        .expect("app_access_token missing")
        .to_string()
}

/// Get Feishu WebSocket URL via /callback/ws/endpoint (official SDK approach).
/// Returns the full WSS URL with embedded auth ticket.
async fn get_feishu_ws_url(app_id: &str, app_secret: &str) -> String {
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(FEISHU_WS_ENDPOINT_URL)
        .header("locale", "zh")
        .json(&serde_json::json!({"AppID": app_id, "AppSecret": app_secret}))
        .send()
        .await
        .expect("ws endpoint request failed")
        .json()
        .await
        .expect("ws endpoint JSON parse failed");
    assert_eq!(
        resp["code"].as_i64().unwrap_or(-1),
        0,
        "Feishu ws endpoint failed: {resp:?}"
    );
    resp["data"]["URL"]
        .as_str()
        .expect("ws URL missing in response")
        .to_string()
}

#[tokio::test]
#[ignore = "requires LARK_APP_ID + LARK_APP_SECRET - run with: cargo test -p qai-server --test e2e_lark -- --ignored --nocapture"]
async fn test_lark_auth_and_connect() {
    let app_id = match std::env::var("LARK_APP_ID") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("SKIP test_lark_auth_and_connect: LARK_APP_ID not set");
            return;
        }
    };
    let app_secret = match std::env::var("LARK_APP_SECRET") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("SKIP test_lark_auth_and_connect: LARK_APP_SECRET not set");
            return;
        }
    };

    // Test 1: get_access_token succeeds
    let token = get_feishu_token(&app_id, &app_secret).await;
    assert!(!token.is_empty(), "app_access_token is empty");
    println!("✓ Feishu app_access_token obtained (len={})", token.len());

    // Test 2: get WebSocket URL via /callback/ws/endpoint
    let ws_url = get_feishu_ws_url(&app_id, &app_secret).await;
    assert!(!ws_url.is_empty(), "ws_url is empty");
    assert!(ws_url.starts_with("wss://"), "ws_url is not wss: {ws_url}");
    println!("✓ Feishu ws_url obtained");

    // Install ring as the default CryptoProvider for rustls 0.23 (required by tokio-tungstenite 0.26)
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Test 3: WebSocket connects successfully
    let ws_result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio_tungstenite::connect_async(&ws_url),
    )
    .await;

    match ws_result {
        Ok(Ok((mut ws, _))) => {
            println!("✓ Feishu WebSocket connected");
            let _ = ws.close(None).await;
        }
        Ok(Err(e)) => panic!("Feishu WS connect failed: {e}"),
        Err(_) => panic!("Feishu WS connect timed out after 10s"),
    }

    println!("test_lark_auth_and_connect PASSED");
}

#[tokio::test]
#[ignore = "requires LARK_APP_ID + LARK_APP_SECRET + LARK_TEST_OPEN_ID"]
async fn test_lark_send_message() {
    let app_id = match std::env::var("LARK_APP_ID") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("SKIP test_lark_send_message: LARK_APP_ID not set");
            return;
        }
    };
    let app_secret = match std::env::var("LARK_APP_SECRET") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("SKIP test_lark_send_message: LARK_APP_SECRET not set");
            return;
        }
    };
    let open_id = match std::env::var("LARK_TEST_OPEN_ID") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("SKIP test_lark_send_message: LARK_TEST_OPEN_ID not set");
            return;
        }
    };

    let token = get_feishu_token(&app_id, &app_secret).await;
    let client = reqwest::Client::new();

    // Send a DM to the test user
    let send_resp: serde_json::Value = client
        .post(format!(
            "{FEISHU_BASE}/im/v1/messages?receive_id_type=open_id"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "receive_id": open_id,
            "msg_type": "text",
            "content": "{\"text\":\"[QuickAI E2E Test] Hello from Rust gateway test\"}"
        }))
        .send()
        .await
        .expect("Feishu send request failed")
        .json()
        .await
        .expect("JSON parse failed");

    println!("Feishu send DM response: {send_resp:?}");
    assert_eq!(
        send_resp["code"].as_i64().unwrap_or(-1),
        0,
        "Feishu send failed: {send_resp:?}"
    );
    println!("test_lark_send_message PASSED");
}
