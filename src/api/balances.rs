//! Balance endpoints.

use crate::client::RevolutXClient;
use crate::error::Result;
use crate::model::balances::Balance;
use crate::transport::RequestSpec;

/// Account balance endpoints, reached via [`RevolutXClient::balances`].
pub struct BalancesApi<'a> {
    client: &'a RevolutXClient,
}

impl<'a> BalancesApi<'a> {
    pub(crate) fn new(client: &'a RevolutXClient) -> Self {
        Self { client }
    }

    /// `GET /balances` — returns all currency balances for the account.
    pub async fn get_all(&self) -> Result<Vec<Balance>> {
        self.client.send_json(RequestSpec::get("/balances")).await
    }
}
