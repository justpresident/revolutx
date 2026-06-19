//! Exchange configuration endpoints.

use crate::client::RevolutXClient;
use crate::error::Result;
use crate::model::configuration::{Currencies, CurrencyPairs};
use crate::transport::RequestSpec;

/// Exchange configuration endpoints, reached via
/// [`RevolutXClient::configuration`].
pub struct ConfigurationApi<'a> {
    client: &'a RevolutXClient,
}

impl<'a> ConfigurationApi<'a> {
    pub(crate) fn new(client: &'a RevolutXClient) -> Self {
        Self { client }
    }

    /// `GET /configuration/currencies` — returns the supported currencies keyed
    /// by currency code.
    pub async fn currencies(&self) -> Result<Currencies> {
        self.client
            .transport()
            .send_json(RequestSpec::get("/configuration/currencies"))
            .await
    }

    /// `GET /configuration/pairs` — returns the supported trading pairs keyed by
    /// pair code.
    pub async fn pairs(&self) -> Result<CurrencyPairs> {
        self.client
            .transport()
            .send_json(RequestSpec::get("/configuration/pairs"))
            .await
    }
}
