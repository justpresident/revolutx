//! The `RequestExecutor` seam: a custom executor can drive the full client API
//! without any HTTP. This is the pattern the future agent-backed executor uses —
//! the client builds a `RequestSpec`, the executor returns a `RawResponse`, and
//! the client parses it into typed results / classified errors.

use std::sync::{Arc, Mutex};

use revolutx::RevolutXClient;
use revolutx::transport::{BoxFuture, RawResponse, RequestExecutor, RequestSpec};

/// Records the request it received and returns a canned response.
struct StubExecutor {
    response: RawResponse,
    last: Mutex<Option<(String, String)>>,
}

impl RequestExecutor for StubExecutor {
    fn execute<'a>(&'a self, request: RequestSpec) -> BoxFuture<'a, revolutx::Result<RawResponse>> {
        *self.last.lock().unwrap() = Some((
            request.method().as_str().to_string(),
            request.path().to_string(),
        ));
        let response = self.response.clone();
        Box::pin(async move { Ok(response) })
    }

    fn base_url(&self) -> &str {
        "http://agent.local/api/1.0"
    }

    fn is_authenticated(&self) -> bool {
        true
    }
}

fn client_with(response: RawResponse) -> (RevolutXClient, Arc<StubExecutor>) {
    let executor = Arc::new(StubExecutor {
        response,
        last: Mutex::new(None),
    });
    (RevolutXClient::with_executor(executor.clone()), executor)
}

#[tokio::test]
async fn custom_executor_drives_the_client_api() {
    let body = r#"[{"currency":"BTC","available":"1.25","reserved":"0.10","total":"1.35"}]"#;
    let (client, executor) = client_with(RawResponse {
        status: 200,
        retry_after: None,
        body: body.as_bytes().to_vec(),
    });

    // Client metadata is delegated to the executor.
    assert!(client.is_authenticated());
    assert_eq!(client.base_url(), "http://agent.local/api/1.0");

    // The normal endpoint API works end-to-end over the custom executor.
    let balances = client.balances().get_all().await.unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].currency, "BTC");

    // The executor received the request the SDK built.
    let (method, path) = executor.last.lock().unwrap().clone().unwrap();
    assert_eq!(method, "GET");
    assert_eq!(path, "/balances");
}

#[tokio::test]
async fn custom_executor_error_responses_are_classified() {
    let body = r#"{"message":"Rate Limit Exceeded","error_id":"x","timestamp":1}"#;
    let (client, _) = client_with(RawResponse {
        status: 429,
        retry_after: Some(std::time::Duration::from_millis(5000)),
        body: body.as_bytes().to_vec(),
    });

    let err = client.balances().get_all().await.unwrap_err();
    assert!(err.is_rate_limited());
    assert_eq!(
        err.retry_after(),
        Some(std::time::Duration::from_millis(5000))
    );
}
