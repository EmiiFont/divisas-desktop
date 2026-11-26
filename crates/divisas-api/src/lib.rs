//! Client for the [divisas-cloud](https://github.com/EmiiFont/divisas-cloud) API.
//!
//! The backend is a Hono service (`apps/api`) deployed on Railway. REST routes:
//!
//! - `GET /healthz` — no API key required
//! - `GET /v1/rates/latest?country=&currency=`
//! - `GET /v1/banks?country=`
//! - `GET /v1/banks/{key}`
//! - `GET /v1/rates/history/{key}?from=&to=`
//! - `GET /v1/official?date=`
//! - `GET /v1/averages?country=&currency=`
//! - `GET /v1/remittance/quotes?targetCountry=&sourceCountry=&amount=&limit=`
//!
//! All `/v1/*` routes require an `X-API-Key` header. Responses use the
//! envelope `{ "data": ..., "meta": ... }`; errors are JSON with
//! `{ "error": { "code", "message" } }`.

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Railway deployment URL — override with `DIVISAS_API_BASE_URL` if it changes.
pub const DEFAULT_BASE_URL: &str = "https://divisas-api-production-9a6a0cb6.up.railway.app";

/// Firebase web API key (Identity Toolkit) used to exchange email + password
/// for an ID token. Public client identifier; override with
/// `DIVISAS_WEB_API_KEY` if the project's key changes.
pub const DEFAULT_WEB_API_KEY: &str = "AIzaSyAbL6EFTvi-WrdJ512eIOzNkibZhB8jvSI";

/// Google Identity Toolkit endpoint for email + password sign-in.
pub const IDENTITY_TOOLKIT_URL: &str =
    "https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword";

/// Google Identity Toolkit endpoint for anonymous sign-in (signUp without
/// email/password creates an anonymous user).
pub const IDENTITY_TOOLKIT_SIGNUP_URL: &str =
    "https://identitytoolkit.googleapis.com/v1/accounts:signUp";

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
                // Return responses for all statuses so error bodies can be
                // parsed instead of collapsing into Error::StatusCode.
                .http_status_as_error(false)
                .build()
                .into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    // ------------------------------------------------------------------
    // Endpoints
    // ------------------------------------------------------------------

    /// `GET /healthz` — no API key required.
    pub fn health(&self) -> Result<HealthResponse, ApiError> {
        self.get_json("healthz", &[])
    }

    /// `GET /v1/rates/latest` — flat best-rate rows for one currency across banks.
    ///
    /// Params: `country` (required, e.g. `DO`), `currency` (optional, default USD).
    pub fn rates_latest(&self, country: &str, currency: Option<&str>) -> Result<RatesLatestResponse, ApiError> {
        let mut params = vec![("country", country)];
        if let Some(currency) = currency {
            params.push(("currency", currency));
        }
        self.get_json("v1/rates/latest", &params)
    }

    /// `GET /v1/banks` — banks with full rate lists for a country.
    pub fn banks(&self, country: &str) -> Result<BanksResponse, ApiError> {
        self.get_json("v1/banks", &[("country", country)])
    }

    /// `GET /v1/banks/{key}` — single bank by key.
    pub fn bank_detail(&self, key: &str) -> Result<BankDetailResponse, ApiError> {
        let path = format!("v1/banks/{}", urlencode(key));
        self.get_json(&path, &[])
    }

    /// `GET /v1/rates/history/{key}` — history points for a bank key.
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
        let path = format!("v1/rates/history/{}", urlencode(key));
        self.get_json(&path, &params)
    }

    /// `GET /v1/official` — Banco Central reference rates for (or before) a date.
    pub fn official(&self, date: Option<&str>) -> Result<OfficialResponse, ApiError> {
        let params = date.map(|d| [("date", d)]).unwrap_or_default();
        self.get_json("v1/official", &params)
    }

    /// `GET /v1/averages` — market averages per country/currency.
    pub fn averages(&self, country: Option<&str>, currency: Option<&str>) -> Result<AveragesResponse, ApiError> {
        let mut params = Vec::new();
        if let Some(country) = country {
            params.push(("country", country));
        }
        if let Some(currency) = currency {
            params.push(("currency", currency));
        }
        self.get_json("v1/averages", &params)
    }

    /// `GET /v1/remittance/quotes` — remittance quotes for a corridor.
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
        self.get_json("v1/remittance/quotes", &params)
    }

    /// `POST /v1/keys` — exchange a Firebase ID token for a fresh API key.
    ///
    /// The raw key is returned exactly once and only its hash is stored
    /// server-side. Re-calling revokes any previous key for the owner.
    pub fn request_api_key(&self, id_token: &str) -> Result<ApiKeyIssued, ApiError> {
        let url = format!("{}/v1/keys", self.base_url);
        let mut response = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {id_token}"))
            .send_json(&serde_json::json!({}))?;

        let status = response.status();
        let body: serde_json::Value = response.body_mut().read_json()?;
        if !status.is_success() {
            return Err(json_error_body(&body, status.as_u16()));
        }
        serde_json::from_value(body).map_err(ApiError::from)
    }

    /// Sign in anonymously against Firebase Identity Toolkit and immediately
    /// mint an API key. The backend must allow anonymous users for the key
    /// issuance to succeed.
    pub fn sign_in_anonymous_and_issue_key(&self, web_api_key: &str) -> Result<ApiKeyIssued, ApiError> {
        let id_token = sign_in_anonymous(web_api_key)?;
        self.request_api_key(&id_token)
    }

    /// Sign in with email + password against Firebase Identity Toolkit and
    /// immediately mint an API key. Convenience wrapper over [`Self::sign_in`]
    /// and [`Self::request_api_key`].
    pub fn sign_in_and_issue_key(
        &self,
        email: &str,
        password: &str,
        web_api_key: &str,
    ) -> Result<ApiKeyIssued, ApiError> {
        let id_token = sign_in_password(email, password, web_api_key)?;
        self.request_api_key(&id_token)
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
        if path != "healthz" {
            request = request.header("X-API-Key", &self.api_key);
        }

        let mut response = request.call()?;

        let status = response.status();
        if !status.is_success() {
            let error: ErrorBody = response.body_mut().read_json().unwrap_or(ErrorBody {
                error: ErrorDetail {
                    code: "http-error".into(),
                    message: format!("HTTP {} from {url}", status.as_u16()),
                },
            });
            return Err(ApiError::Api {
                status: status.as_u16(),
                code: error.error.code,
                message: error.error.message,
            });
        }

        let body: Envelope<T> = response.body_mut().read_json()?;
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

// ----------------------------------------------------------------------
// Auth: Firebase Identity Toolkit + API key issuance
// ----------------------------------------------------------------------

/// Result of `POST /v1/keys` — the raw key is shown exactly once.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyIssued {
    pub api_key: String,
    pub key_prefix: String,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub daily_quota: Option<u32>,
    #[serde(default)]
    pub warning: Option<String>,
}

/// Exchange email + password for a Firebase ID token via Google's
/// Identity Toolkit REST endpoint (needs the project's web API key).
pub fn sign_in_password(
    email: &str,
    password: &str,
    web_api_key: &str,
) -> Result<String, ApiError> {
    let url = format!("{IDENTITY_TOOLKIT_URL}?key={web_api_key}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent.post(&url).send_json(&serde_json::json!({
        "email": email,
        "password": password,
        "returnSecureToken": true,
    }))?;

    let status = response.status();
    let body: serde_json::Value = response.body_mut().read_json()?;
    if !status.is_success() {
        return Err(json_error_body(&body, status.as_u16()));
    }
    body.get("idToken")
        .and_then(|token| token.as_str())
        .map(|token| token.to_string())
        .ok_or_else(|| ApiError::Api {
            status: status.as_u16(),
            code: "missing-id-token".into(),
            message: "Sign-in response did not contain an idToken.".into(),
        })
}

/// Sign in anonymously (create an anonymous Firebase user) and return the
/// ID token.
pub fn sign_in_anonymous(web_api_key: &str) -> Result<String, ApiError> {
    sign_up_anonymous(web_api_key)
}

fn sign_up_anonymous(web_api_key: &str) -> Result<String, ApiError> {
    let url = format!("{IDENTITY_TOOLKIT_SIGNUP_URL}?key={web_api_key}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(&url)
        .send_json(&serde_json::json!({ "returnSecureToken": true }))?;
    let status = response.status();
    let body: serde_json::Value = response.body_mut().read_json()?;
    if !status.is_success() {
        return Err(json_error_body(&body, status.as_u16()));
    }
    body.get("idToken")
        .and_then(|token| token.as_str())
        .map(|token| token.to_string())
        .ok_or_else(|| ApiError::Api {
            status: status.as_u16(),
            code: "missing-id-token".into(),
            message: "Sign-up response did not contain an idToken.".into(),
        })
}

/// Parse the backend's `{ error: { code, message } }` body.
fn json_error_body(body: &serde_json::Value, status: u16) -> ApiError {
    let detail = body.get("error").cloned().unwrap_or(serde_json::Value::Null);
    let (code, message) = match detail {
        serde_json::Value::Object(map) => (
            map.get("code").and_then(|v| v.as_str()).unwrap_or("http-error").to_string(),
            map.get("message").and_then(|v| v.as_str()).unwrap_or("Request failed.").to_string(),
        ),
        // Identity Toolkit nests error differently.
        _ => (
            body.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("http-error")
                .to_string(),
            body.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("Request failed.")
                .to_string(),
        ),
    };
    ApiError::Api { status, code, message }
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    code: String,
    message: String,
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
            let Ok(average) = serde_json::from_value::<MarketAverageEntry>(average.clone()) else {
                continue;
            };
            rows.push(MarketAverage {
                country: country.clone(),
                currency: currency.clone(),
                avg_buy: average.avg_buy,
                avg_sale: average.avg_sale,
                avg_spread: average.avg_spread,
            });
        }
    }
    rows.sort_by(|a, b| (&a.country, &a.currency).cmp(&(&b.country, &b.currency)));
    rows
}

#[derive(Debug, Deserialize)]
struct MarketAverageEntry {
    #[serde(rename = "avgBuy")]
    avg_buy: f64,
    #[serde(rename = "avgSale")]
    avg_sale: f64,
    #[serde(rename = "avgSpread")]
    #[serde(default)]
    avg_spread: Option<f64>,
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

impl RemittanceQuote {
    /// Effective rate: local currency received per USD sent, i.e.
    /// `amount_received / send_amount`. Returns `(rate, amount_received)`.
    pub fn effective_rate(&self) -> Option<(f64, f64)> {
        let sent = self.send_amount?;
        let received = self.amount_received?;
        if sent <= 0.0 {
            return None;
        }
        Some((received / sent, received))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remittance_effective_rate() {
        let quote = RemittanceQuote {
            provider: "test".into(),
            name: "Test".into(),
            key: "test".into(),
            source_country: Some("US".into()),
            target_country: Some("DO".into()),
            source_currency: Some("USD".into()),
            target_currency: Some("DOP".into()),
            exchange_rate: Some(62.5),
            fee_usd: Some(2.99),
            send_amount: Some(100.0),
            amount_received: Some(6120.0),
            delivery_speed_minutes: Some(15),
            last_updated: None,
        };
        let (rate, received) = quote.effective_rate().unwrap();
        assert_eq!(rate, 61.2);
        assert_eq!(received, 6120.0);

        let empty = RemittanceQuote {
            send_amount: None,
            ..quote
        };
        assert!(empty.effective_rate().is_none());
    }

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

    #[test]
    fn error_body_shape() {
        let body = r#"{
            "error": { "code": "missing-api-key", "message": "Provide an API key via the X-API-Key header." }
        }"#;
        let parsed: ErrorBody = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.error.code, "missing-api-key");
    }

    #[test]
    fn averages_shape() {
        let body = r#"{
            "data": {
                "generatedAt": "2026-01-01T00:00:00Z",
                "countries": {
                    "DO": {
                        "USD": { "avgBuy": 62.1, "avgSale": 62.9, "avgSpread": 0.8 },
                        "EUR": { "avgBuy": 67.0, "avgSale": 68.0 }
                    }
                }
            },
            "meta": {}
        }"#;
        let parsed: Envelope<AveragesResponse> = serde_json::from_str(body).unwrap();
        let rows = flatten_averages(&parsed.data);
        assert_eq!(rows.len(), 2);
        // Sorted by country then currency: EUR before USD.
        assert_eq!(rows[0].currency, "EUR");
        assert_eq!(rows[0].avg_buy, 67.0);
        assert_eq!(rows[1].currency, "USD");
        assert_eq!(rows[1].avg_buy, 62.1);
        assert!(rows[1].avg_spread.is_some());
        assert!(rows[0].avg_spread.is_none());
    }
}
