//! Client for the [divisas-cloud](https://github.com/EmiiFont/divisas-cloud) public API.
//!
//! The backend is a set of Firebase Cloud Functions (project `bankpricesstore`).
//! Each endpoint is a standalone function reachable at:
//!
//! `https://<region>-bankpricesstore.cloudfunctions.net/<functionName>`
//!
//! All endpoints are GET-only and require an `X-API-Key` header. Responses use
//! the envelope `{ "data": ..., "meta": ... }`; errors are JSON with
//! `{ "error": { "code", "message" } }` (or a flattened `code`/`message`).

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const DEFAULT_BASE_URL: &str = "https://us-central1-bankpricesstore.cloudfunctions.net";

pub const COUNTRIES: &[&str] = &["DO", "CO", "PE", "GT", "CR", "NI", "MX"];
pub const CURRENCIES: &[&str] = &["USD", "EUR", "CHF", "GBP", "CAD"];

/// Client for the divisas-cloud public API.
#[derive(Clone)]
pub struct DivisasClient {
    agent: ureq::Agent,
    base_url: String,
    api_key: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("HTTP {status}: {code} — {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("network error: {0}")]
    Network(#[from] ureq::Error),
    #[error("invalid response body: {0}")]
    Parse(#[from] serde_json::Error),
}

impl DivisasClient {
    /// Create a client. `api_key` is required by every endpoint except health.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .build()
                .into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    // ------------------------------------------------------------------
    // Endpoints
    // ------------------------------------------------------------------

    /// `apiHealth` — no API key required.
    pub fn health(&self) -> Result<HealthResponse, ApiError> {
        self.get_json("apiHealth", &[])
    }

    /// `apiRatesLatest` — flat best-rate rows for one currency across banks.
    ///
    /// Params: `country` (required, e.g. `DO`), `currency` (optional, default USD).
    pub fn rates_latest(&self, country: &str, currency: Option<&str>) -> Result<RatesLatestResponse, ApiError> {
        let mut params = vec![("country", country)];
        if let Some(currency) = currency {
            params.push(("currency", currency));
        }
        self.get_json("apiRatesLatest", &params)
    }

    /// `apiBanks` — banks with full rate lists for a country.
    pub fn banks(&self, country: &str) -> Result<BanksResponse, ApiError> {
        self.get_json("apiBanks", &[("country", country)])
    }

    /// `apiBankDetail` — single bank by key (first path segment).
    pub fn bank_detail(&self, key: &str) -> Result<BankDetailResponse, ApiError> {
        let path = format!("apiBankDetail/{}", key);
        self.get_json(&path, &[])
    }

    /// `apiRatesHistory` — history points for a bank key.
    pub fn rates_history(
        &self,
        key: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<RatesHistoryResponse, ApiError> {
        let mut params = Vec::new();
        if let Some(from) = from {
            params.push(("from", from));
        }
        if let Some(to) = to {
            params.push(("to", to));
        }
        let path = format!("apiRatesHistory/{}", key);
        self.get_json(&path, &params)
    }

    /// `apiOfficial` — Banco Central reference rates for (or before) a date.
    pub fn official(&self, date: Option<&str>) -> Result<OfficialResponse, ApiError> {
        let params = date.map(|d| [("date", d)]).unwrap_or_default();
        self.get_json("apiOfficial", &params)
    }

    /// `apiAverages` — market averages per country/currency.
    pub fn averages(&self, country: Option<&str>, currency: Option<&str>) -> Result<AveragesResponse, ApiError> {
        let mut params = Vec::new();
        if let Some(country) = country {
            params.push(("country", country));
        }
        if let Some(currency) = currency {
            params.push(("currency", currency));
        }
        self.get_json("apiAverages", &params)
    }

    /// `apiRemittanceQuotes` — remittance quotes for a corridor.
    ///
    /// `target_country` is required (e.g. `DO`); `source_country` defaults to `US`.
    pub fn remittance_quotes(
        &self,
        target_country: &str,
        source_country: Option<&str>,
        amount: Option<f64>,
        limit: Option<u32>,
    ) -> Result<RemittanceQuotesResponse, ApiError> {
        let mut params = vec![("targetCountry", target_country)];
        if let Some(source) = source_country {
            params.push(("sourceCountry", source));
        }
        let amount_string = amount.map(|amount| format!("{amount}"));
        if let Some(amount) = amount_string.as_deref() {
            params.push(("amount", amount));
        }
        let limit_string = limit.map(|limit| format!("{limit}"));
        if let Some(limit) = limit_string.as_deref() {
            params.push(("limit", limit));
        }
        self.get_json("apiRemittanceQuotes", &params)
    }

    // ------------------------------------------------------------------
    // Plumbing
    // ------------------------------------------------------------------

    fn get_json<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> Result<T, ApiError> {
        let mut url = format!("{}/{}", self.base_url, path);
        if !params.is_empty() {
            let query: Vec<String> = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencode(v)))
                .collect();
            url.push('?');
            url.push_str(&query.join("&"));
        }

        let mut request = self.agent.get(&url);
        if path != "apiHealth" {
            request = request.header("X-API-Key", &self.api_key);
        }

        let body: Envelope<T> = request.call()?.body_mut().read_json()?;
        Ok(body.data)
    }
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ----------------------------------------------------------------------
// Response envelope
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub cache: Option<String>,
    #[serde(default)]
    pub generated_at: Option<String>,
}

// ----------------------------------------------------------------------
// Endpoint payloads
// ----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatesLatestResponse {
    pub country: String,
    pub currency: String,
    pub count: usize,
    #[serde(default)]
    pub as_of_date: Option<String>,
    pub rates: Vec<RateRow>,
}

/// One row of `apiRatesLatest` output: best rates per bank for one currency.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateRow {
    pub key: String,
    pub name: String,
    pub currency: String,
    pub buy: f64,
    pub sell: f64,
    #[serde(default)]
    pub spread: Option<f64>,
    /// "increase" | "decrease" | "equal"
    #[serde(default)]
    pub buy_change: Option<String>,
    #[serde(default)]
    pub sell_change: Option<String>,
    #[serde(default)]
    pub last_updated: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BanksResponse {
    pub country: String,
    pub count: usize,
    pub banks: Vec<Bank>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bank {
    pub id: String,
    pub key: String,
    pub name: String,
    pub country: String,
    #[serde(default)]
    pub institution_type: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub last_updated: Option<String>,
    pub rates: Vec<Rate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rate {
    pub currency: String,
    pub buy: f64,
    pub sell: f64,
    #[serde(default)]
    pub spread: Option<f64>,
    /// "increase" | "decrease" | "equal"
    #[serde(default)]
    pub buy_change: Option<String>,
    #[serde(default)]
    pub sell_change: Option<String>,
}

/// `apiBankDetail` returns the bank object itself as `data`.
pub type BankDetailResponse = Bank;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatesHistoryResponse {
    pub bank: HistoryBank,
    pub from: String,
    pub to: String,
    pub count: usize,
    pub points: Vec<HistoryPoint>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryBank {
    pub id: String,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub country: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub scraped_at: Option<String>,
    pub rates: Vec<Rate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialResponse {
    pub requested_date: String,
    pub effective_date: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub source: Option<OfficialSource>,
    pub rates: Vec<OfficialRate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialSource {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OfficialRate {
    pub currency: String,
    pub buy: f64,
    pub sell: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AveragesResponse {
    #[serde(default)]
    pub generated_at: Option<String>,
    /// `{ country -> { currency -> average } }`
    pub countries: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAverage {
    pub country: String,
    pub currency: String,
    #[serde(rename = "avgBuy")]
    pub avg_buy: f64,
    #[serde(rename = "avgSale")]
    pub avg_sale: f64,
    #[serde(rename = "avgSpread")]
    #[serde(default)]
    pub avg_spread: Option<f64>,
}

/// Flatten the `countries` map into rows sorted by country then currency.
pub fn flatten_averages(response: &AveragesResponse) -> Vec<MarketAverage> {
    let mut rows = Vec::new();
    for (country, currencies) in &response.countries {
        let Some(currencies) = currencies.as_object() else { continue };
        for (currency, average) in currencies {
            let Ok(average) = serde_json::from_value::<MarketAverage>(average.clone()) else {
                continue;
            };
            rows.push(MarketAverage {
                country: country.clone(),
                currency: currency.clone(),
                ..average
            });
        }
    }
    rows.sort_by(|a, b| (&a.country, &a.currency).cmp(&(&b.country, &b.currency)));
    rows
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemittanceQuotesResponse {
    pub source_country: String,
    pub target_country: String,
    #[serde(default)]
    pub amount: Option<f64>,
    pub count: usize,
    pub quotes: Vec<RemittanceQuote>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemittanceQuote {
    pub provider: String,
    pub name: String,
    pub key: String,
    #[serde(default)]
    pub source_country: Option<String>,
    #[serde(default)]
    pub target_country: Option<String>,
    #[serde(default)]
    pub source_currency: Option<String>,
    #[serde(default)]
    pub target_currency: Option<String>,
    #[serde(default)]
    pub exchange_rate: Option<f64>,
    #[serde(default)]
    pub fee_usd: Option<f64>,
    #[serde(default)]
    pub send_amount: Option<f64>,
    #[serde(default)]
    pub amount_received: Option<f64>,
    #[serde(default)]
    pub delivery_speed_minutes: Option<u64>,
    #[serde(default)]
    pub last_updated: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_is_unwrapped() {
        let body = r#"{
            "data": {
                "status": "ok",
                "service": "divisas-public-api"
            },
            "meta": { "generatedAt": "2026-01-01T00:00:00Z" }
        }"#;
        let parsed: Envelope<HealthResponse> = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.data.status, "ok");
    }

    #[test]
    fn rates_latest_shape() {
        let body = r#"{
            "data": {
                "country": "DO",
                "currency": "USD",
                "count": 1,
                "asOfDate": "2026-01-01",
                "rates": [{
                    "key": "banco_central",
                    "name": "Banco Central",
                    "currency": "USD",
                    "buy": 62.5,
                    "sell": 63.0,
                    "buyChange": "increase",
                    "sellChange": "equal"
                }]
            },
            "meta": {}
        }"#;
        let parsed: Envelope<RatesLatestResponse> = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.data.rates[0].buy_change.as_deref(), Some("increase"));
    }
}
