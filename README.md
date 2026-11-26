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

The backend is a Hono service (`apps/api` in divisas-cloud) deployed on Railway. REST routes, all GET-only, authenticated with `X-API-Key`:

| Route | Purpose | Params |
|---|---|---|
| `GET /healthz` | health check | — (no key) |
| `GET /v1/rates/latest` | best rates per bank | `country`, `currency` |
| `GET /v1/banks` | banks + full rate lists | `country` |
| `GET /v1/banks/{key}` | one bank | path: `{key}` |
| `GET /v1/rates/history/{key}` | rate history | path: `{key}`, `from`, `to` |
| `GET /v1/official` | Banco Central reference rates | `date` |
| `GET /v1/averages` | market averages | `country`, `currency` |
| `GET /v1/remittance/quotes` | remittance quotes | `targetCountry`, `sourceCountry`, `amount`, `limit` |

Responses use the `{ data, meta }` envelope; errors are `{ "error": { code, message } }`. Countries: `DO, CO, PE, GT, CR, NI, MX`; currencies: `USD, EUR, CHF, GBP, CAD`.

## Prerequisites

- Rust 1.85+ (`rustup`)
- Xcode + command line tools on macOS (`sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`)
- A divisas-cloud API key (issued via the backend's `requestApiKey` flow)

## Running

```sh
export DIVISAS_WEB_API_KEY="<AIza... web API key from Firebase project settings>"  # for sign-in
# optional: export DIVISAS_API_BASE_URL="https://divisas-api-production-9a6a0cb6.up.railway.app"
cargo run -p divisas-ui
```

### Sign-in flow

There is no shared API key. On first launch the app shows a login screen:

1. Email + password → Firebase Identity Toolkit (`accounts:signInWithPassword`) → ID token.
2. `POST /v1/keys` with `Authorization: Bearer <id-token>` → a fresh free-tier API key (shown once, stored server-side only as a hash; re-issuing revokes the previous key).
3. The key is stored in the OS keychain (service `divisas-desktop`). Subsequent launches skip the login screen.

`DIVISAS_API_KEY` in the environment bypasses the login flow entirely.

## Features

- Login screen: email/password → Firebase Identity Toolkit → `POST /v1/keys` → key stored in the OS keychain
- Tabs: **Rates** (best buy/sell per bank), **Banks**, **Official** (Banco Central), **Averages**, **Remittance** quotes
- Country + currency chip filters
- Blocking HTTP (`ureq`) executed on GPUI's background executor, results applied back to the entity on the main thread
- `Cmd-R` refresh, `Cmd-Q` quit

## Testing

```sh
cargo test --workspace
```

## macOS prerequisites

GPUI compiles Metal shaders at build time, so Xcode (not just Command Line Tools) is required:

```sh
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
```
