# Divisas Desktop

A native macOS/desktop GUI client for the [divisas-cloud](https://github.com/EmiiFont/divisas-cloud) backend, built with [GPUI](https://gpui.rs) (Zed's GPU-accelerated UI framework).

## Layout

```
divisas-desktop/
├── Cargo.toml            # cargo workspace
└── crates/
    ├── divisas-api/      # typed client for the divisas-cloud public API
    └── divisas-ui/       # GPUI desktop app (binary: divisas-desktop)
```

## Backend contract

The backend is Firebase Cloud Functions (project `bankpricesstore`); each endpoint is a standalone HTTPS function requiring `X-API-Key` (GET only):

| Function | Purpose | Params |
|---|---|---|
| `apiHealth` | health check | — (no key) |
| `apiRatesLatest` | best rates per bank | `country`, `currency` |
| `apiBanks` | banks + full rate lists | `country` |
| `apiBankDetail` | one bank | path: `{key}` |
| `apiRatesHistory` | rate history | path: `{key}`, `from`, `to` |
| `apiOfficial` | Banco Central reference rates | `date` |
| `apiAverages` | market averages | `country`, `currency` |
| `apiRemittanceQuotes` | remittance quotes | `targetCountry`, `sourceCountry`, `amount`, `limit` |

Responses use the `{ data, meta }` envelope. Countries: `DO, CO, PE, GT, CR, NI, MX`; currencies: `USD, EUR, CHF, GBP, CAD`.

## Prerequisites

- Rust 1.85+ (`rustup`)
- Xcode + command line tools on macOS (`sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`)
- A divisas-cloud API key (issued via the backend's `requestApiKey` flow)

## Running

```sh
export DIVISAS_API_KEY="your-key"
# optional: export DIVISAS_API_BASE_URL="https://<region>-bankpricesstore.cloudfunctions.net"
cargo run -p divisas-ui
```

## Features

- Tabs: **Rates** (best buy/sell per bank), **Banks**, **Official** (Banco Central), **Averages**, **Remittance** quotes
- Country + currency chip filters
- Blocking HTTP (`ureq`) executed on GPUI's background executor, results applied back to the entity on the main thread
- `Cmd-R` refresh, `Cmd-Q` quit

## Testing

```sh
cargo test --workspace
```
