//! Divisas Desktop — GPUI client for the divisas-cloud API.
//!
//! On first launch a login screen collects email + password, signs in against
//! Firebase Identity Toolkit (needs `DIVISAS_WEB_API_KEY`), mints an API key
//! via `POST /v1/keys`, and stores it in the OS keychain. Later launches load
//! the key straight from the keychain (or `DIVISAS_API_KEY`).

mod session;
mod text_input;
mod theme;

use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Application, Background, Bounds,
    ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable, KeyBinding, Path, PathBuilder,
    Pixels, Render, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, actions,
    canvas, div, point, prelude::*, px, rgb, size, white,
};

use divisas_api::{ApiKeyIssued, ApiError, Bank, COUNTRIES, CURRENCIES, DivisasClient, HistoryPoint, MarketAverage, RateRow, RemittanceQuote, flatten_averages};
use text_input::TextInput;
use theme::Palette;
use theme::{DOWN, DOWN_SOFT};

// ----------------------------------------------------------------------
// Actions
// ----------------------------------------------------------------------

actions!(divisas, [Refresh, Quit, SwitchTab1, SwitchTab2, SwitchTab3, SwitchTab4, SwitchTab5, SwitchTab6, SignOut]);

// ----------------------------------------------------------------------
// State
// ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Rates,
    Banks,
    Official,
    Averages,
    Remittance,
    Calculator,
}

impl Tab {
    fn label(&self) -> &'static str {
        match self {
            Tab::Rates => "Rates",
            Tab::Banks => "Banks",
            Tab::Official => "Official",
            Tab::Averages => "Averages",
            Tab::Remittance => "Remittance",
            Tab::Calculator => "Calculator",
        }
    }

    /// Single-glyph icon for the rail buttons.
    fn glyph(&self) -> &'static str {
        match self {
            Tab::Rates => "⇅",
            Tab::Banks => "▤",
            Tab::Official => "⚖",
            Tab::Averages => "∑",
            Tab::Remittance => "⇄",
            Tab::Calculator => "±",
        }
    }

    const ALL: [Tab; 6] = [
        Tab::Rates,
        Tab::Banks,
        Tab::Official,
        Tab::Averages,
        Tab::Remittance,
        Tab::Calculator,
    ];
}

#[derive(Clone, Debug)]
enum Data {
    Empty,
    Loading,
    Error(String),
    Rates(Vec<RateRow>),
    Banks(Vec<Bank>),
    Averages(Vec<MarketAverage>),
    Remittance(Vec<RemittanceQuote>),
    /// Latest rates for every currency of the selected country, for the
    /// exchange calculator.
    Calculator(Vec<RateRow>),
}

/// Direction of the exchange calculation.
/// Calculator state passed down from the app during render (reading the
/// app entity inside a render pass would panic — GPUI forbids nested reads).
struct CalcContext {
    amount_text: String,
    direction: CalcDirection,
    currency: &'static str,
    country: &'static str,
    calc_amount: Entity<TextInput>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CalcDirection {
    /// Foreign currency → DOP: banks buy your foreign currency.
    ForeignToDop,
    /// DOP → foreign currency: banks sell you foreign currency.
    DopToForeign,
}

/// State of the right-side detail drawer for one bank.
struct Drawer {
    key: String,
    name: String,
    currency: String,
    state: DrawerState,
}

#[allow(dead_code)]
enum DrawerState {
    Loading,
    Error(String),
    Ready {
        detail: Option<Bank>,
        history: Vec<HistoryPoint>,
    },
}

enum Screen {
    /// Asking for credentials; `error` shown on failure, `busy` while signing in.
    Login {
        email: Entity<TextInput>,
        password: Entity<TextInput>,
        error: Option<String>,
        busy: bool,
    },
    Main,
}

/// How the current session was authenticated. Guest sessions can be
/// restored silently; email sessions cannot (we never store passwords).
#[derive(Clone, Copy, PartialEq)]
enum AuthMethod {
    Email,
    Anonymous,
}

struct DivisasApp {
    client: DivisasClient,
    web_api_key: String,
    base_url: String,
    auth_method: AuthMethod,
    focus_handle: FocusHandle,
    screen: Screen,
    tab: Tab,
    country: usize,
    currency: usize,
    data: Data,
    status: Option<SharedString>,
    drawer: Option<Drawer>,
    /// Guards against re-auth loops when the API keeps rejecting the key.
    reauth_attempts: u32,
    /// Free-text search that filters the current tab's rows.
    search: Entity<TextInput>,
    /// Calculator: amount being converted.
    calc_amount: Entity<TextInput>,
    /// Calculator: direction of the exchange.
    calc_direction: CalcDirection,
    /// Rate history for the chart (best-buy bank of the selected currency).
    history: Option<HistoryState>,
    /// Chart time range in days (7 or 30).
    history_days: u32,
    /// Which "About" accordion rows are expanded.
    about_open: [bool; 3],
}

/// Async rate-history fetch for the chart.
#[derive(Clone)]
struct HistoryState {
    /// Bank name the series belongs to.
    bank: String,
    /// None while loading, Some(Err) on failure.
    result: Option<Result<Vec<HistoryPoint>, String>>,
}

impl DivisasApp {
    fn country(&self) -> &'static str {
        COUNTRIES[self.country]
    }

    fn currency(&self) -> &'static str {
        CURRENCIES[self.currency]
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.data = Data::Loading;
        self.status = Some("Loading…".into());
        // History is refetched with the tab; drop the old series.
        if self.tab == Tab::Rates {
            self.history = None;
        }
        cx.notify();

        let client = self.client.clone();
        let tab = self.tab;
        let country = self.country().to_string();
        let currency = self.currency().to_string();

        // ureq is blocking; run it on the background executor and come back
        // into the entity to apply the result.
        let task = cx.background_executor().spawn(async move {
            let result = match tab {
                Tab::Rates => client
                    .rates_latest(&country, Some(&currency))
                    .map(|r| Data::Rates(r.rates)),
                Tab::Banks => client.banks(&country).map(|r| Data::Banks(r.banks)),
                Tab::Official => client.official(None).map(|r| Data::Rates(
                    r.rates
                        .into_iter()
                        .map(|rate| RateRow {
                            key: "banco_central".into(),
                            name: "Banco Central".into(),
                            currency: rate.currency,
                            buy: rate.buy,
                            sell: rate.sell,
                            spread: None,
                            buy_change: None,
                            sell_change: None,
                            last_updated: None,
                        })
                        .collect(),
                )),
                Tab::Averages => client
                    .averages(Some(&country), Some(&currency))
                    .map(|r| Data::Averages(flatten_averages(&r))),
                Tab::Remittance => client
                    .remittance_quotes(&country, None, None, Some(10))
                    .map(|r| Data::Remittance(r.quotes)),
                Tab::Calculator => {
                    // Latest rates for every currency of the country, so the
                    // calculator can convert against all of them at once.
                    let mut rows = Vec::new();
                    for currency in CURRENCIES {
                        match client.rates_latest(&country, Some(currency)) {
                            Ok(mut response) => rows.append(&mut response.rates),
                            Err(ApiError::Api { status: 404, .. }) => {}
                            Err(err) => return Err(err),
                        }
                    }
                    Ok(Data::Calculator(rows))
                }
            };
            match result {
                Ok(data) => Ok(data),
                Err(ApiError::Api { status, code, message }) => Err(ApiError::Api {
                    status,
                    code,
                    message,
                }),
                Err(err) => Err(err),
            }
        });

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |app, cx| {
                app.status = None;
                match &result {
                    Ok(data) => {
                        app.reauth_attempts = 0;
                        // When rates load, fetch history for the best-buy
                        // bank of the selected currency to power the chart.
                        if tab == Tab::Rates && matches!(data, Data::Rates(_)) {
                            app.fetch_history(cx);
                        }
                        app.data = data.clone();
                    }
                    // A rejected key is a session problem, not a data problem:
                    // silently re-authenticate (guest) or return to login.
                    Err(ApiError::Api { status: 401, .. }) => app.silent_reauth(cx),
                    Err(err) => app.data = Data::Error(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fetch rate history for the chart: the best-buy bank of the selected
    /// currency, over the current `history_days` range.
    fn fetch_history(&mut self, cx: &mut Context<Self>) {
        let Data::Rates(rows) = &self.data else {
            return;
        };
        // The bank with the best buy rate — the series the chart tracks.
        let Some(best) = rows
            .iter()
            .filter(|row| row.currency == self.currency())
            .filter(|row| row.buy.is_finite())
            .max_by(|a, b| a.buy.partial_cmp(&b.buy).unwrap_or(std::cmp::Ordering::Equal))
        else {
            return;
        };

        let client = self.client.clone();
        let key = best.key.clone();
        let bank = best.name.clone();
        let days = self.history_days;
        let from = theme::iso_date_days_ago(days as u64);
        self.history = Some(HistoryState {
            bank: bank.clone(),
            result: None,
        });
        cx.notify();

        let task = cx.background_executor().spawn(async move {
            client
                .rates_history(&key, Some(&from), None)
                .map(|response| response.points)
                .map_err(|err| err.to_string())
        });

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |app, cx| {
                // Ignore stale results if the range changed mid-flight.
                if app.history_days == days {
                    app.history = Some(HistoryState {
                        bank,
                        result: Some(result),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Set the chart time range and refetch history only.
    fn set_history_days(&mut self, days: u32, cx: &mut Context<Self>) {
        if self.history_days == days {
            return;
        }
        self.history_days = days;
        self.fetch_history(cx);
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        let Screen::Login { email, password, error, busy } = &mut self.screen else {
            return;
        };
        *error = None;
        if *busy {
            return;
        }
        let email_text = email.read(cx).content.to_string();
        let password_text = password.read(cx).content.to_string();
        if email_text.trim().is_empty() || password_text.is_empty() {
            *error = Some("Enter your email and password.".into());
            cx.notify();
            return;
        }
        let web_api_key = self.web_api_key.clone();
        self.auth_method = AuthMethod::Email;

        *busy = true;
        cx.notify();

        let client = self.client.clone();
        let task = cx.background_executor().spawn(async move {
            client.sign_in_and_issue_key(&email_text, &password_text, &web_api_key)
        });

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |app, cx| {
                app.finish_sign_in(result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn sign_in_anonymous(&mut self, cx: &mut Context<Self>) {
        let Screen::Login { error, busy, .. } = &mut self.screen else {
            return;
        };
        *error = None;
        if *busy {
            return;
        }
        *busy = true;
        self.auth_method = AuthMethod::Anonymous;
        cx.notify();

        let client = self.client.clone();
        let web_api_key = self.web_api_key.clone();
        let task = cx.background_executor().spawn(async move {
            client.sign_in_anonymous_and_issue_key(&web_api_key)
        });

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |app, cx| {
                app.finish_sign_in(result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn finish_sign_in(&mut self, result: Result<ApiKeyIssued, ApiError>, cx: &mut Context<Self>) {
        match result {
            Ok(issued) => {
                if let Err(err) = session::save_api_key(&issued.api_key) {
                    self.status = Some(format!("Signed in, but could not store key: {err}").into());
                }
                session::save_auth_method(match self.auth_method {
                    AuthMethod::Email => "email",
                    AuthMethod::Anonymous => "anonymous",
                });
                self.client = DivisasClient::with_base_url(issued.api_key, self.base_url.clone());
                self.reauth_attempts = 0;
                self.screen = Screen::Main;
                self.fetch(cx);
            }
            Err(ApiError::Api { status, code, message }) => {
                if let Screen::Login { error, busy, .. } = &mut self.screen {
                    *busy = false;
                    *error = Some(format!("HTTP {status} — {code}: {message}"));
                }
            }
            Err(err) => {
                if let Screen::Login { error, busy, .. } = &mut self.screen {
                    *busy = false;
                    *error = Some(err.to_string());
                }
            }
        }
        cx.notify();
    }

    /// Clear the stored session and return to the login screen.
    fn sign_out(&mut self, cx: &mut Context<Self>) {
        let _ = session::clear_api_key();
        self.data = Data::Empty;
        self.status = None;
        self.drawer = None;
        self.reauth_attempts = 0;
        self.client = DivisasClient::with_base_url("", self.base_url.clone());
        self.screen = Screen::Login {
            email: cx.new(|cx| TextInput::new("", "email@example.com", false, cx)),
            password: cx.new(|cx| TextInput::new("", "password", true, cx)),
            error: None,
            busy: false,
        };
        cx.notify();
    }

    /// Issue a fresh key without user input (guest sessions only) and retry.
    fn silent_reauth(&mut self, cx: &mut Context<Self>) {
        if self.auth_method != AuthMethod::Anonymous || self.reauth_attempts >= 2 {
            // Email sessions need the password; fall back to the login screen.
            if let Screen::Main = self.screen {
                self.screen = Screen::Login {
                    email: cx.new(|cx| TextInput::new("", "email@example.com", false, cx)),
                    password: cx.new(|cx| TextInput::new("", "password", true, cx)),
                    error: Some("Session expired. Sign in again.".into()),
                    busy: false,
                };
            }
            cx.notify();
            return;
        }
        self.reauth_attempts += 1;
        self.status = Some("Restoring session…".into());
        let client = self.client.clone();
        let web_api_key = self.web_api_key.clone();
        let task = cx
            .background_executor()
            .spawn(async move { client.sign_in_anonymous_and_issue_key(&web_api_key) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |app, cx| {
                app.status = None;
                match result {
                    Ok(issued) => {
                        let _ = session::save_api_key(&issued.api_key);
                        session::save_auth_method("anonymous");
                        app.client = DivisasClient::with_base_url(
                            issued.api_key,
                            app.base_url.clone(),
                        );
                        app.fetch(cx);
                    }
                    Err(_) => {
                        app.screen = Screen::Login {
                            email: cx.new(|cx| TextInput::new("", "email@example.com", false, cx)),
                            password: cx.new(|cx| TextInput::new("", "password", true, cx)),
                            error: Some("Session expired. Sign in again.".into()),
                            busy: false,
                        };
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.fetch(cx);
    }

    fn set_country(&mut self, index: usize, cx: &mut Context<Self>) {
        self.country = index;
        self.fetch(cx);
    }

    fn set_currency(&mut self, index: usize, cx: &mut Context<Self>) {
        self.currency = index;
        self.fetch(cx);
    }

    /// Open the bank detail drawer and fetch detail + history in background.
    fn open_drawer(&mut self, key: String, name: String, cx: &mut Context<Self>) {
        let currency = self.currency().to_string();
        self.drawer = Some(Drawer {
            key: key.clone(),
            name,
            currency: currency.clone(),
            state: DrawerState::Loading,
        });
        cx.notify();

        let client = self.client.clone();
        let key_for_fetch = key.clone();
        let task = cx.background_executor().spawn(async move {
            let detail = client.bank_detail(&key_for_fetch).ok();
            let history = client
                .rates_history(&key_for_fetch, None, None)
                .map(|response| response.points)
                .unwrap_or_default();
            (detail, history)
        });

        cx.spawn(async move |this, cx| {
            let (detail, history) = task.await;
            this.update(cx, |app, cx| {
                if let Some(drawer) = &mut app.drawer {
                    if drawer.key == key {
                        drawer.state = DrawerState::Ready { detail, history };
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn close_drawer(&mut self, cx: &mut Context<Self>) {
        self.drawer = None;
        cx.notify();
    }
}

impl Focusable for DivisasApp {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DivisasApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.screen {
            Screen::Login { .. } => self.render_login(cx).into_any_element(),
            Screen::Main => self.render_main(cx).into_any_element(),
        }
    }
}

/// Freshest `lastUpdated`-style timestamp across the current data variant.
fn freshest_updated(data: &Data) -> Option<String> {
    let mut freshest: Option<String> = None;
    let mut consider = |candidate: Option<&String>| {
        if let Some(ts) = candidate {
            if freshest
                .as_ref()
                .and_then(|current| theme::minutes_since(current))
                .map_or(true, |current_min| {
                    theme::minutes_since(ts)
                        .map_or(false, |candidate_min| candidate_min < current_min)
                })
            {
                freshest = Some(ts.clone());
            }
        }
    };
    match data {
        Data::Rates(rows) => rows.iter().for_each(|row| consider(row.last_updated.as_ref())),
        Data::Banks(banks) => banks
            .iter()
            .for_each(|bank| consider(bank.last_updated.as_ref())),
        Data::Remittance(quotes) => quotes
            .iter()
            .for_each(|quote| consider(quote.last_updated.as_ref())),
        _ => {}
    }
    freshest
}

/// "Updated 3 min ago" plus a stale flag when data is older than 30 min.
fn updated_label(data: &Data) -> Option<(bool, String)> {
    let iso = freshest_updated(data)?;
    let minutes = theme::minutes_since(&iso)?;
    Some((theme::is_stale(minutes), theme::time_ago(minutes)))
}

impl DivisasApp {
    fn render_login(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Screen::Login { email, password, error, busy } = &self.screen else {
            unreachable!()
        };
        let email = email.clone();
        let password = password.clone();
        let error = error.clone();
        let busy = *busy;

        let password_submit = password.clone();
        let email_focus = email.clone();

        div()
            .track_focus(&self.focus_handle)
            .key_context("login")
            .on_action(cx.listener(|app, _: &crate::text_input::Submit, _window, cx| {
                app.sign_in(cx);
            }))
            .on_action(|_: &Quit, _window, cx| cx.quit())
            .bg(rgb(0x0B1220))
            .text_color(rgb(0xE5E7EB))
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_8()
                    .rounded(px(12.))
                    .bg(rgb(0x111827))
                    .w(px(360.))
                    .child(
                        div()
                            .text_size(px(22.))
                            .text_color(white())
                            .child("Divisas Desktop"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(0x9CA3AF))
                            .child("Sign in to mint your API key. Stored in the keychain."),
                    )
                    .child(
                        div()
                            .id("email-box")
                            .p_2()
                            .rounded(px(6.))
                            .bg(rgb(0x1F2937))
                            .border_1()
                            .border_color(rgb(0x374151))
                            .on_click(cx.listener(move |app, _: &ClickEvent, window, cx| {
                                if let Screen::Login { email, .. } = &app.screen {
                                    window.focus(&email.read(cx).focus_handle(cx));
                                }
                            }))
                            // Render the TextInput entity itself: it owns the
                            // action handlers (backspace, arrows, paste) and
                            // key context. A bare InputField would only paint.
                            .child(email_focus),
                    )
                    .child(
                        div()
                            .id("password-box")
                            .p_2()
                            .rounded(px(6.))
                            .bg(rgb(0x1F2937))
                            .border_1()
                            .border_color(rgb(0x374151))
                            .on_click(cx.listener(move |app, _: &ClickEvent, window, cx| {
                                if let Screen::Login { password, .. } = &app.screen {
                                    window.focus(&password.read(cx).focus_handle(cx));
                                }
                            }))
                            .child(password_submit),
                    )
                    .child(
                        div()
                            .id("sign-in")
                            .px_4()
                            .py_2()
                            .rounded(px(6.))
                            .bg(rgb(0x2563EB))
                            .text_color(white())
                            .text_size(px(14.))
                            .cursor_pointer()
                            .hover(|this| this.opacity(0.9))
                            .child(if busy { "Signing in…" } else { "Sign in" })
                            .on_click(cx.listener(|app, _: &ClickEvent, _window, cx| {
                                app.sign_in(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("guest")
                            .px_4()
                            .py_2()
                            .rounded(px(6.))
                            .bg(rgb(0x1F2937))
                            .text_color(rgb(0x9CA3AF))
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|this| this.opacity(0.9))
                            .child(if busy { "Working…" } else { "Continue as guest" })
                            .on_click(cx.listener(|app, _: &ClickEvent, _window, cx| {
                                app.sign_in_anonymous(cx);
                            })),
                    )
                    .children(error.clone().map(|message| {
                        div()
                            .p_3()
                            .rounded(px(6.))
                            .bg(rgb(0x7F1D1D))
                            .text_color(rgb(0xFCA5A5))
                            .text_size(px(12.))
                            .child(message)
                    }))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x6B7280))
                            .child("Enter submits. \"Continue as guest\" signs in anonymously (the backend must allow anonymous API keys)."),
                    ),
            )
    }

    fn render_main(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab;
        let country = self.country();
        let currency = self.currency();

        // ---------------------------------------------------- Icon rail
        let rail_items = Tab::ALL
            .map(|candidate| {
                let selected = candidate == tab;
                div()
                    .id(SharedString::from(format!("rail-{}", candidate.label())))
                    .size(px(40.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(17.))
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(rgb(Palette::ACCENT)).text_color(rgb(Palette::WHITE))
                    })
                    .when(!selected, |this| {
                        this.text_color(rgb(Palette::TEXT_TERTIARY)).hover(|style| {
                            style
                                .bg(rgb(Palette::SURFACE_HOVER))
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                        })
                    })
                    .child(candidate.glyph())
                    .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                        app.set_tab(candidate, cx);
                    }))
            })
            .into_iter()
            .collect::<Vec<_>>();

        // Segmented country + currency controls (navbar)
        let country_segments = COUNTRIES
            .iter()
            .enumerate()
            .map(|(index, code)| {
                let selected = index == self.country;
                div()
                    .id(SharedString::from(format!("country-{code}")))
                    .px_3()
                    .py_1()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .text_size(px(theme::TEXT_SM))
                    .when(selected, |this| {
                        this.bg(rgb(Palette::ACCENT)).text_color(rgb(Palette::WHITE))
                    })
                    .when(!selected, |this| {
                        this.text_color(rgb(Palette::TEXT_SECONDARY))
                            .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                    })
                    .child(*code)
                    .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                        app.set_country(index, cx);
                    }))
            })
            .collect::<Vec<_>>();

        let currency_segments = CURRENCIES
            .iter()
            .enumerate()
            .map(|(index, code)| {
                let selected = index == self.currency;
                div()
                    .id(SharedString::from(format!("currency-{code}")))
                    .px_3()
                    .py_1()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .text_size(px(theme::TEXT_SM))
                    .when(selected, |this| {
                        this.bg(rgb(Palette::ACCENT)).text_color(rgb(Palette::WHITE))
                    })
                    .when(!selected, |this| {
                        this.text_color(rgb(Palette::TEXT_SECONDARY))
                            .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                    })
                    .child(*code)
                    .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                        app.set_currency(index, cx);
                    }))
            })
            .collect::<Vec<_>>();

        div()
            .track_focus(&self.focus_handle)
            .key_context("divisas")
            .on_action(cx.listener(|app, _: &Refresh, _window, cx| app.fetch(cx)))
            .on_action(cx.listener(|app, _: &SwitchTab1, _window, cx| {
                app.set_tab(Tab::ALL[0], cx);
            }))
            .on_action(cx.listener(|app, _: &SwitchTab2, _window, cx| {
                app.set_tab(Tab::ALL[1], cx);
            }))
            .on_action(cx.listener(|app, _: &SwitchTab3, _window, cx| {
                app.set_tab(Tab::ALL[2], cx);
            }))
            .on_action(cx.listener(|app, _: &SwitchTab4, _window, cx| {
                app.set_tab(Tab::ALL[3], cx);
            }))
            .on_action(cx.listener(|app, _: &SwitchTab5, _window, cx| {
                app.set_tab(Tab::ALL[4], cx);
            }))
            .on_action(cx.listener(|app, _: &SwitchTab6, _window, cx| {
                app.set_tab(Tab::ALL[5], cx);
            }))
            .on_action(cx.listener(|app, _: &SignOut, _window, cx| app.sign_out(cx)))
            .on_action(|_: &Quit, _window, cx| cx.quit())
            .bg(rgb(Palette::ROOT))
            .text_color(rgb(Palette::TEXT))
            .size_full()
            .flex()
            .flex_row()
            // ------------------------------------------------ Icon rail
            .child(
                div()
                    .w(px(64.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(theme::SPACE_LG))
                    .bg(rgb(Palette::SIDEBAR))
                    .border_r_1()
                    .border_color(rgb(Palette::BORDER))
                    .py(px(theme::SPACE_LG))
                    .child(
                        // Logo: violet gradient circle with a currency glyph
                        div()
                            .size(px(40.))
                            .rounded_full()
                            .bg(theme::accent_card_gradient())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(18.))
                            .text_color(rgb(Palette::WHITE))
                            .child("¤"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(theme::SPACE_SM))
                            .children(rail_items),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_end()
                            .child(
                                div()
                                    .id("rail-sign-out")
                                    .size(px(40.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(15.))
                                    .text_color(rgb(Palette::TEXT_TERTIARY))
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style
                                            .bg(rgb(Palette::SURFACE_HOVER))
                                            .text_color(rgb(Palette::TEXT_SECONDARY))
                                    })
                                    .child("⎋")
                                    .on_click(cx.listener(|app, _: &ClickEvent, _window, cx| {
                                        app.sign_out(cx);
                                    })),
                            ),
                    ),
            )
            // ------------------------------------------------ Left column
            .child(self.render_left_column(cx))
            // ------------------------------------------------ Main column
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        // Navbar: segmented switchers, search, refresh pill
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px(px(theme::SPACE_2XL))
                            .py(px(theme::SPACE_LG))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(theme::SPACE_SM))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(theme::SPACE_XS))
                                            .p(px(theme::SPACE_XS))
                                            .rounded(px(theme::RADIUS_MD))
                                            .bg(rgb(Palette::SURFACE))
                                            .border_1()
                                            .border_color(rgb(Palette::BORDER))
                                            .children(country_segments),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(theme::SPACE_XS))
                                            .p(px(theme::SPACE_XS))
                                            .rounded(px(theme::RADIUS_MD))
                                            .bg(rgb(Palette::SURFACE))
                                            .border_1()
                                            .border_color(rgb(Palette::BORDER))
                                            .children(currency_segments),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(theme::SPACE_SM))
                                    .child(
                                        div()
                                            .id("search-box")
                                            .w(px(220.))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py(px(theme::SPACE_XS))
                                            .rounded(px(theme::RADIUS_MD))
                                            .bg(rgb(Palette::SURFACE))
                                            .border_1()
                                            .border_color(rgb(Palette::BORDER))
                                            .text_size(px(theme::TEXT_SM))
                                            .on_click(cx.listener(
                                                |app, _: &ClickEvent, window, cx| {
                                                    window.focus(
                                                        &app.search.read(cx).focus_handle(cx),
                                                    );
                                                },
                                            ))
                                            .child(
                                                div()
                                                    .text_color(rgb(Palette::TEXT_TERTIARY))
                                                    .child("⌕"),
                                            )
                                            .child(self.search.clone()),
                                    )
                                    .child(
                                        div()
                                            .id("refresh")
                                            .px_4()
                                            .py(px(theme::SPACE_SM))
                                            .rounded(px(theme::RADIUS_MD))
                                            .bg(rgb(Palette::ACCENT))
                                            .text_size(px(theme::TEXT_SM))
                                            .text_color(rgb(Palette::WHITE))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(Palette::ACCENT_HOVER)))
                                            .child("Refresh")
                                            .on_click(cx.listener(
                                                |app, _: &ClickEvent, _window, cx| {
                                                    app.fetch(cx);
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("content")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .max_w(px(880.))
                                    .px(px(theme::SPACE_2XL))
                                    .pb(px(theme::SPACE_2XL))
                                    .child(render_body(
                                        &self.data,
                                        &self.search.read(cx).content.to_string(),
                                        &CalcContext {
                                            amount_text: self
                                                .calc_amount
                                                .read(cx)
                                                .content
                                                .to_string(),
                                            direction: self.calc_direction,
                                            currency: self.currency(),
                                            country: self.country(),
                                            calc_amount: self.calc_amount.clone(),
                                        },
                                        self.tab,
                                        self.history.as_ref(),
                                        self.history_days,
                                        cx,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(theme::SPACE_SM))
                            .px(px(theme::SPACE_2XL))
                            .py(px(theme::SPACE_MD))
                            .border_t_1()
                            .border_color(rgb(Palette::BORDER))
                            .children(updated_label(&self.data).map(|(stale, label)| {
                                let dot = if stale { Palette::WARN } else { Palette::UP };
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(theme::TEXT_XS))
                                    .text_color(rgb(if stale {
                                        Palette::WARN
                                    } else {
                                        Palette::TEXT_SECONDARY
                                    }))
                                    .child(
                                        div()
                                            .size(px(7.))
                                            .rounded_full()
                                            .bg(rgb(dot)),
                                    )
                                    .child(format!("Updated {label}"))
                            }))
                            .child(self.status.clone().unwrap_or_else(|| {
                                format!("{country} · {currency} · divisas-cloud API").into()
                            })),
                    ),
            )
            // ------------------------------------------------ Detail drawer
            .children(
                self.drawer
                    .as_ref()
                    .map(|drawer| render_drawer(drawer, cx))
                    .into_iter(),
            )
    }

    /// Persistent left column: highlight card, market snapshot, data status,
    /// and informational accordions (the DeFi dashboard's left card stack).
    fn render_left_column(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("left-column")
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_LG))
            .px(px(theme::SPACE_LG))
            .py(px(theme::SPACE_XL))
            .overflow_y_scroll()
            .child(self.render_highlight_card(cx))
            .child(self.render_snapshot_card(cx))
            .child(self.render_status_card(cx))
            .children(self.render_about_rows(cx))
            .into_any_element()
    }

    /// Amber highlight card nudging the user toward the calculator.
    fn render_highlight_card(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .rounded(px(theme::RADIUS_LG))
            .bg(rgb(Palette::AMBER_SOFT))
            .border_1()
            .border_color(rgb(Palette::AMBER))
            .p(px(theme::SPACE_LG))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_SM))
            .child(
                div()
                    .size(px(32.))
                    .rounded_full()
                    .bg(rgb(Palette::AMBER))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.))
                    .text_color(rgb(Palette::WHITE))
                    .child("±"),
            )
            .child(
                div()
                    .text_size(px(theme::TEXT_BASE))
                    .text_color(rgb(Palette::TEXT))
                    .child("Convert in seconds"),
            )
            .child(
                div()
                    .text_size(px(theme::TEXT_XS))
                    .text_color(rgb(Palette::TEXT_SECONDARY))
                    .child("Pick a currency and amount to see what every bank pays."),
            )
            .child(
                div()
                    .id("open-calculator")
                    .mt(px(theme::SPACE_XS))
                    .px_3()
                    .py_1()
                    .rounded(px(theme::RADIUS_SM))
                    .bg(rgb(Palette::AMBER))
                    .text_size(px(theme::TEXT_SM))
                    .text_color(rgb(Palette::WHITE))
                    .cursor_pointer()
                    .child("Open Calculator")
                    .on_click(cx.listener(|app, _: &ClickEvent, _window, cx| {
                        app.set_tab(Tab::Calculator, cx);
                    })),
            )
            .into_any_element()
    }

    /// Market snapshot card: best buy / best sell / average for the pair.
    fn render_snapshot_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let (country, currency) = (self.country(), self.currency());

        div()
            .rounded(px(theme::RADIUS_LG))
            .bg(rgb(Palette::SURFACE))
            .border_1()
            .border_color(rgb(Palette::BORDER))
            .p(px(theme::SPACE_LG))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_SM))
            .child(
                div()
                    .text_size(px(theme::TEXT_SM))
                    .text_color(rgb(Palette::TEXT_SECONDARY))
                    .child("Market snapshot"),
            )
            .child(
                div()
                    .text_size(px(theme::TEXT_XS))
                    .text_color(rgb(Palette::TEXT_TERTIARY))
                    .child(format!("{country} · {currency}")),
            )
            .children(self.snapshot_rows(cx))
            .into_any_element()
    }

    fn snapshot_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let _ = cx;
        let Data::Rates(rows) = &self.data else {
            return vec![div()
                .text_size(px(theme::TEXT_XS))
                .text_color(rgb(Palette::TEXT_TERTIARY))
                .child("Open the Rates tab to see market data.")
                .into_any_element()];
        };

        let matching: Vec<&RateRow> = rows
            .iter()
            .filter(|row| row.currency == self.currency())
            .collect();
        if matching.is_empty() {
            return vec![div()
                .text_size(px(theme::TEXT_XS))
                .text_color(rgb(Palette::TEXT_TERTIARY))
                .child("No rates for this pair yet.")
                .into_any_element()];
        }

        let best_buy = matching
            .iter()
            .max_by(|a, b| a.buy.partial_cmp(&b.buy).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        let best_sell = matching
            .iter()
            .min_by(|a, b| a.sell.partial_cmp(&b.sell).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        let avg = matching.iter().map(|r| r.buy).sum::<f64>() / matching.len() as f64;

        vec![
            snapshot_row("Best buy", theme::format_rate(best_buy.buy), best_buy.name.clone()),
            snapshot_row("Best sell", theme::format_rate(best_sell.sell), best_sell.name.clone()),
            snapshot_row("Market avg", theme::format_rate(avg), format!("{} banks", matching.len())),
        ]
    }

    /// Violet gradient card with the freshness indicator and a refresh button.
    fn render_status_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let (stale, label) = updated_label(&self.data).unwrap_or((false, "—".into()));
        let dot = if stale { Palette::WARN } else { Palette::UP };

        div()
            .rounded(px(theme::RADIUS_LG))
            .bg(theme::accent_soft_gradient())
            .border_1()
            .border_color(rgb(Palette::BORDER))
            .p(px(theme::SPACE_LG))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_SM))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(theme::SPACE_SM))
                    .child(div().size(px(8.)).rounded_full().bg(rgb(dot)))
                    .child(
                        div()
                            .text_size(px(theme::TEXT_XS))
                            .text_color(rgb(Palette::TEXT_SECONDARY))
                            .child(format!("Updated {label}")),
                    ),
            )
            .child(
                div()
                    .text_size(px(theme::TEXT_XS))
                    .text_color(rgb(Palette::TEXT_TERTIARY))
                    .child("Rates refresh when you switch tabs or pairs."),
            )
            .child(
                div()
                    .id("left-refresh")
                    .px_3()
                    .py_1()
                    .rounded(px(theme::RADIUS_SM))
                    .bg(rgb(Palette::ACCENT))
                    .text_size(px(theme::TEXT_SM))
                    .text_color(rgb(Palette::WHITE))
                    .cursor_pointer()
                    .child("Refresh rates")
                    .on_click(cx.listener(|app, _: &ClickEvent, _window, cx| {
                        app.fetch(cx);
                    })),
            )
            .into_any_element()
    }

    /// Informational accordion rows ("How are rates collected?" etc.).
    fn render_about_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        const ABOUT: [(&str, &str); 3] = [
            (
                "How are rates collected?",
                "Rates are scraped from bank websites several times a day and published by the divisas-cloud API.",
            ),
            (
                "Why do rates differ between banks?",
                "Each bank sets its own buy and sell prices; the spread between them is how currency exchange earns.",
            ),
            (
                "What is the official rate?",
                "The Banco Central publishes a reference rate used for official invoicing and customs.",
            ),
        ];

        ABOUT
            .iter()
            .enumerate()
            .map(|(index, (title, body))| {
                let open = self.about_open[index];
                div()
                    .rounded(px(theme::RADIUS_MD))
                    .bg(rgb(Palette::SURFACE))
                    .border_1()
                    .border_color(rgb(Palette::BORDER))
                    .overflow_hidden()
                    .child(
                        div()
                            .id(SharedString::from(format!("about-{index}")))
                            .px_3()
                            .py_2()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                            .child(
                                div()
                                    .text_size(px(theme::TEXT_SM))
                                    .text_color(rgb(Palette::TEXT))
                                    .child(*title),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TEXT_XS))
                                    .text_color(rgb(Palette::TEXT_TERTIARY))
                                    .child(if open { "−" } else { "+" }),
                            )
                            .on_click(cx.listener(move |app, _: &ClickEvent, _window, cx| {
                                app.about_open[index] = !app.about_open[index];
                                cx.notify();
                            })),
                    )
                    .when(open, |this| {
                        this.child(
                            div()
                                .px_3()
                                .pb_3()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                                .child(*body),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }
}

/// Right-side drawer showing bank detail + rate history.
fn render_drawer(drawer: &Drawer, cx: &mut Context<DivisasApp>) -> AnyElement {
    let header = div()
        .flex()
        .flex_row()
        .justify_between()
        .items_center()
        .px(px(theme::SPACE_LG))
        .py(px(theme::SPACE_LG))
        .border_b_1()
        .border_color(rgb(Palette::BORDER))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(theme::TEXT_MD))
                        .text_color(rgb(Palette::TEXT))
                        .child(drawer.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_TERTIARY))
                        .child(drawer.currency.clone()),
                ),
        )
        .child(
            div()
                .id("drawer-close")
                .px_2()
                .py_1()
                .rounded(px(theme::RADIUS_SM))
                .text_size(px(theme::TEXT_SM))
                .text_color(rgb(Palette::TEXT_SECONDARY))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                .child("✕")
                .on_click(cx.listener(|app, _event: &ClickEvent, _window, cx| {
                    app.close_drawer(cx);
                })),
        );

    let body: AnyElement = match &drawer.state {
        DrawerState::Loading => skeleton_list(3),
        DrawerState::Error(message_text) => message_box(message_text, DOWN_SOFT, DOWN),
        DrawerState::Ready { detail, history } => {
            let mut section = div().flex().flex_col().gap(px(theme::SPACE_LG)).p(px(theme::SPACE_LG));

            if let Some(bank) = detail {
                let mut rates_section = div().flex().flex_col().gap(px(theme::SPACE_SM));
                rates_section = rates_section.child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child("CURRENT RATES"),
                );
                for rate in &bank.rates {
                    rates_section = rates_section.child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .p(px(theme::SPACE_MD))
                            .rounded(px(theme::RADIUS_SM))
                            .bg(rgb(Palette::SURFACE))
                            .border_1()
                            .border_color(rgb(Palette::BORDER))
                            .child(
                                div()
                                    .text_size(px(theme::TEXT_SM))
                                    .text_color(rgb(Palette::TEXT_SECONDARY))
                                    .child(rate.currency.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(theme::SPACE_LG))
                                    .child(
                                        div()
                                            .text_size(px(theme::TEXT_BASE))
                                            .text_color(rgb(Palette::UP))
                                            .child(format!("buy {}", theme::format_rate(rate.buy))),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::TEXT_BASE))
                                            .text_color(rgb(Palette::DOWN))
                                            .child(format!("sell {}", theme::format_rate(rate.sell))),
                                    ),
                            ),
                    );
                }
                section = section.child(rates_section);
            }

            if history.is_empty() {
                section = section.child(
                    div()
                        .text_size(px(theme::TEXT_SM))
                        .text_color(rgb(Palette::TEXT_TERTIARY))
                        .child("No recent history available."),
                );
            } else {
                let mut history_section = div().flex().flex_col().gap(px(theme::SPACE_SM));
                history_section = history_section.child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child("RECENT HISTORY"),
                );
                for point in history.iter().rev().take(10) {
                    let date = point.date.clone().unwrap_or_else(|| "—".into());
                    let buy = point
                        .rates
                        .iter()
                        .find(|rate| rate.currency == drawer.currency)
                        .map(|rate| theme::format_rate(rate.buy));
                    history_section = history_section.child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .text_size(px(theme::TEXT_SM))
                            .child(
                                div()
                                    .text_color(rgb(Palette::TEXT_SECONDARY))
                                    .child(theme::short_date(&date)),
                            )
                            .child(
                                div()
                                    .text_color(rgb(Palette::TEXT))
                                    .child(buy.unwrap_or_else(|| "—".into())),
                            ),
                    );
                }
                section = section.child(history_section);
            }

            section.into_any_element()
        }
    };

    div()
        .w(px(340.))
        .flex()
        .flex_col()
        .bg(rgb(Palette::SIDEBAR))
        .border_l_1()
        .border_color(rgb(Palette::BORDER))
        .child(header)
        .child(
            div()
                .id("drawer-body")
                .flex_1()
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
}

fn render_body(
    data: &Data,
    search: &str,
    calc: &CalcContext,
    tab: Tab,
    history: Option<&HistoryState>,
    history_days: u32,
    cx: &mut Context<DivisasApp>,
) -> AnyElement {
    // Case-insensitive substring match over the row's searchable text.
    let needle = search.to_lowercase();
    let matches = |haystack: &str| haystack.to_lowercase().contains(&needle);
    match data {
        Data::Empty => empty_state(tab, cx),
        Data::Loading => skeleton_list(6),
        Data::Error(message) => error_state(message, cx),
        Data::Rates(rows) => {
            let filtered: Vec<RateRow> = rows
                .iter()
                .filter(|row| {
                    needle.is_empty()
                        || matches(&row.name)
                        || matches(&row.key)
                        || matches(&row.currency)
                })
                .cloned()
                .collect();
            if filtered.is_empty() && !needle.is_empty() {
                no_matches(search)
            } else {
                rates_view(&filtered, history, history_days, cx)
            }
        }
        Data::Banks(banks) => {
            let filtered: Vec<Bank> = banks
                .iter()
                .filter(|bank| needle.is_empty() || matches(&bank.name) || matches(&bank.key))
                .cloned()
                .collect();
            if filtered.is_empty() && !needle.is_empty() {
                no_matches(search)
            } else {
                banks_view(&filtered)
            }
        }
        Data::Averages(rows) => {
            let filtered: Vec<MarketAverage> = rows
                .iter()
                .filter(|row| {
                    needle.is_empty() || matches(&row.country) || matches(&row.currency)
                })
                .cloned()
                .collect();
            if filtered.is_empty() && !needle.is_empty() {
                no_matches(search)
            } else {
                averages_view(&filtered)
            }
        }
        Data::Remittance(quotes) => {
            let filtered: Vec<RemittanceQuote> = quotes
                .iter()
                .filter(|quote| {
                    needle.is_empty()
                        || matches(&quote.name)
                        || matches(&quote.provider)
                        || matches(&quote.key)
                })
                .cloned()
                .collect();
            if filtered.is_empty() && !needle.is_empty() {
                no_matches(search)
            } else {
                remittance_view(&filtered)
            }
        }
        Data::Calculator(rows) => calculator_view(rows, calc, cx),
    }
}

/// Hint shown when the search filter matches nothing.
fn no_matches(search: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .py(px(theme::SPACE_2XL + theme::SPACE_2XL))
        .child(
            div()
                .text_size(px(theme::TEXT_MD))
                .text_color(rgb(Palette::TEXT))
                .child("No matches"),
        )
        .child(
            div()
                .text_size(px(theme::TEXT_SM))
                .text_color(rgb(Palette::TEXT_TERTIARY))
                .child(format!("Nothing matches “{search}”.")),
        )
        .into_any_element()
}

/// Friendly empty state with a hint tailored to the current tab.
fn empty_state(tab: Tab, cx: &mut Context<DivisasApp>) -> AnyElement {
    let (title, hint) = match tab {
        Tab::Rates => (
            "No rates for this pair yet",
            "Try another country or currency, or check back later.",
        ),
        Tab::Banks => (
            "No banks found",
            "This country has no bank rates published yet.",
        ),
        Tab::Official => (
            "No official rates",
            "The central bank hasn't published rates for this currency yet.",
        ),
        Tab::Averages => (
            "No market averages",
            "Averages appear once enough banks report for this pair.",
        ),
        Tab::Remittance => (
            "No remittance quotes",
            "Try a different destination country.",
        ),
        Tab::Calculator => (
            "No rates to calculate with",
            "This country has no published rates yet.",
        ),
    };
    empty_state_card(title, hint, cx)
}

/// Centered empty-state card with a refresh action.
fn empty_state_card(title: &str, hint: &str, cx: &mut Context<DivisasApp>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::SPACE_MD))
        .py(px(theme::SPACE_2XL + theme::SPACE_2XL))
        .child(
            div()
                .size(px(48.))
                .rounded_full()
                .bg(rgb(Palette::SURFACE_SUNKEN))
                .border_1()
                .border_color(rgb(Palette::BORDER))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(theme::TEXT_XL))
                .text_color(rgb(Palette::TEXT_TERTIARY))
                .child("◎"),
        )
        .child(
            div()
                .text_size(px(theme::TEXT_MD))
                .text_color(rgb(Palette::TEXT))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(theme::TEXT_SM))
                .text_color(rgb(Palette::TEXT_TERTIARY))
                .child(hint.to_string()),
        )
        .child(
            div()
                .id("empty-refresh")
                .mt(px(theme::SPACE_SM))
                .px_4()
                .py_2()
                .rounded(px(theme::RADIUS_SM))
                .bg(rgb(Palette::SURFACE))
                .border_1()
                .border_color(rgb(Palette::BORDER))
                .text_size(px(theme::TEXT_SM))
                .text_color(rgb(Palette::ACCENT))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                .child("Refresh")
                .on_click(cx.listener(|app, _event: &ClickEvent, _window, cx| {
                    app.fetch(cx);
                })),
        )
        .into_any_element()
}

/// Error banner with a retry action.
fn error_state(message: &str, cx: &mut Context<DivisasApp>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(theme::SPACE_MD))
        .py(px(theme::SPACE_2XL + theme::SPACE_2XL))
        .child(
            div()
                .max_w(px(560.))
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_SM))
                .p(px(theme::SPACE_LG))
                .rounded(px(theme::RADIUS_MD))
                .bg(rgb(Palette::DOWN_SOFT))
                .border_1()
                .border_color(rgb(Palette::DOWN))
                .child(
                    div()
                        .text_size(px(theme::TEXT_BASE))
                        .text_color(rgb(Palette::DOWN))
                        .child("Couldn't load data"),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_SM))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child(message.to_string()),
                ),
        )
        .child(
            div()
                .id("error-retry")
                .px_4()
                .py_2()
                .rounded(px(theme::RADIUS_SM))
                .bg(rgb(Palette::SURFACE))
                .border_1()
                .border_color(rgb(Palette::BORDER))
                .text_size(px(theme::TEXT_SM))
                .text_color(rgb(Palette::ACCENT))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                .child("Try again")
                .on_click(cx.listener(|app, _event: &ClickEvent, _window, cx| {
                    app.fetch(cx);
                })),
        )
        .into_any_element()
}

/// Metric strip shown above the rates grid.
/// Section heading inside the main content area.
fn section_title(title: &str) -> AnyElement {
    div()
        .text_size(px(theme::TEXT_LG))
        .text_color(rgb(Palette::TEXT))
        .child(title.to_string())
        .into_any_element()
}

/// Violet gradient stat card (the design's hero metrics).
fn gradient_stat_card(label: &str, value: String, detail: String, icon: &str) -> AnyElement {
    div()
        .flex_1()
        .rounded(px(theme::RADIUS_XL))
        .bg(theme::accent_card_gradient())
        .border_1()
        .border_color(rgb(Palette::BORDER))
        .p(px(theme::SPACE_LG))
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_SM))
                .child(
                    div()
                        .size(px(28.))
                        .rounded_full()
                        .bg(rgb(Palette::ACCENT_SOFT))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.))
                        .text_color(rgb(Palette::ACCENT_TEXT))
                        .child(icon.to_string()),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_SM))
                        .text_color(rgb(Palette::ACCENT_TEXT))
                        .child(label.to_string()),
                ),
        )
        .child(
            div()
                .text_size(px(theme::TEXT_DISPLAY))
                .text_color(rgb(Palette::WHITE))
                .child(value),
        )
        .child(
            div()
                .text_size(px(theme::TEXT_XS))
                .text_color(rgb(Palette::TEXT_SECONDARY))
                .child(detail),
        )
        .into_any_element()
}

/// Chart time-range pill (7d / 30d).
fn range_pill(days: u32, current: u32, cx: &mut Context<DivisasApp>) -> AnyElement {
    let selected = days == current;
    div()
        .id(SharedString::from(format!("range-{days}d")))
        .px_3()
        .py_1()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(theme::TEXT_XS))
        .when(selected, |this| {
            this.bg(rgb(Palette::ACCENT)).text_color(rgb(Palette::WHITE))
        })
        .when(!selected, |this| {
            this.text_color(rgb(Palette::TEXT_SECONDARY))
                .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
        })
        .child(format!("{days}d"))
        .on_click(cx.listener(move |app, _: &ClickEvent, _window, cx| {
            app.set_history_days(days, cx);
        }))
        .into_any_element()
}

/// Area chart with a violet gradient fill, drawn with gpui paths.
fn history_chart(points: &[HistoryPoint], currency: &str, bank: &str, height: f32) -> AnyElement {
    // (date, buy) series for the selected currency.
    let series: Vec<(String, f64)> = points
        .iter()
        .filter_map(|point| {
            let rate = point.rates.iter().find(|rate| rate.currency == currency)?;
            Some((point.date.clone().unwrap_or_default(), rate.buy))
        })
        .collect();

    if series.len() < 2 {
        return message_box(
            "Not enough history yet for this pair.",
            rgb(Palette::SURFACE),
            rgb(Palette::TEXT_TERTIARY),
        );
    }

    let min = series.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
    let max = series.iter().map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max);
    let latest = series.last().map(|(_, v)| *v).unwrap_or(0.);

    div()
        .rounded(px(theme::RADIUS_LG))
        .bg(rgb(Palette::SURFACE))
        .border_1()
        .border_color(rgb(Palette::BORDER))
        .p(px(theme::SPACE_MD))
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_SM))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_TERTIARY))
                        .child(format!("Buy rate · {bank} · {currency}")),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_BASE))
                        .text_color(rgb(Palette::ACCENT_TEXT))
                        .child(theme::format_rate(latest)),
                ),
        )
        .child(
            div().h(px(height)).child(canvas(
                move |bounds, _window, _cx| build_chart_paths(bounds, &series, min, max),
                move |_bounds, paths, window, _cx| {
                    for (path, paint) in paths {
                        window.paint_path(path, paint);
                    }
                },
            )),
        )
        .into_any_element()
}

/// Build the chart's fill + line paths in window coordinates.
fn build_chart_paths(
    bounds: Bounds<Pixels>,
    series: &[(String, f64)],
    min: f64,
    max: f64,
) -> Vec<(Path<Pixels>, Background)> {
    let range = (max - min).max(0.0001);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);
    let pad_x = 8.0;
    let pad_top = 10.0;
    let pad_bottom = 6.0;

    let n = series.len();
    let plot_w = width - 2.0 * pad_x;
    let plot_h = height - pad_top - pad_bottom;
    let x_at = |i: usize| origin_x + pad_x + (i as f32 / (n - 1) as f32) * plot_w;
    let y_at = |v: f64| origin_y + pad_top + (1.0 - ((v - min) / range) as f32) * plot_h;

    // Stroke: the rate line.
    let mut line = PathBuilder::stroke(px(2.));
    line.move_to(point(px(x_at(0)), px(y_at(series[0].1))));
    for (index, (_, value)) in series.iter().enumerate().skip(1) {
        line.line_to(point(px(x_at(index)), px(y_at(*value))));
    }
    let line = line.build().ok();

    // Fill: area under the line down to the chart floor.
    let mut fill = PathBuilder::fill();
    fill.move_to(point(px(x_at(0)), px(y_at(series[0].1))));
    for (index, (_, value)) in series.iter().enumerate().skip(1) {
        fill.line_to(point(px(x_at(index)), px(y_at(*value))));
    }
    let floor = origin_y + height;
    fill.line_to(point(px(x_at(n - 1)), px(floor)));
    fill.line_to(point(px(x_at(0)), px(floor)));
    fill.line_to(point(px(x_at(0)), px(y_at(series[0].1))));
    let fill = fill.build().ok();

    let mut paths = Vec::new();
    if let Some(path) = fill {
        paths.push((path, theme::chart_fill_gradient()));
    }
    if let Some(path) = line {
        paths.push((path, rgb(Palette::ACCENT).into()));
    }
    paths
}

/// One row in the left-column market snapshot card.
fn snapshot_row(label: &str, value: String, detail: String) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .py_1()
        .child(
            div()
                .text_size(px(theme::TEXT_XS))
                .text_color(rgb(Palette::TEXT_SECONDARY))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .child(
                    div()
                        .text_size(px(theme::TEXT_SM))
                        .text_color(rgb(Palette::TEXT))
                        .child(value),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(Palette::TEXT_TERTIARY))
                        .child(detail),
                ),
        )
        .into_any_element()
}

fn rates_view(
    rows: &[RateRow],
    history: Option<&HistoryState>,
    history_days: u32,
    cx: &mut Context<DivisasApp>,
) -> AnyElement {
    let best_buy_row = rows
        .iter()
        .filter(|r| r.buy.is_finite())
        .max_by(|a, b| a.buy.partial_cmp(&b.buy).unwrap_or(std::cmp::Ordering::Equal));
    let best_sell_row = rows
        .iter()
        .filter(|r| r.sell.is_finite())
        .min_by(|a, b| a.sell.partial_cmp(&b.sell).unwrap_or(std::cmp::Ordering::Equal));
    let best_buy = best_buy_row.map(|r| r.buy).unwrap_or(0.);
    // The chart series tracks the selected currency.
    let chart_currency = rows.first().map(|r| r.currency.as_str()).unwrap_or("USD");

    div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_2XL))
        // Section: status — two violet gradient stat cards
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_MD))
                .child(section_title("Rates status"))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(theme::SPACE_MD))
                        .child(gradient_stat_card(
                            "Best buy rate",
                            best_buy_row
                                .map(|r| theme::format_rate(r.buy))
                                .unwrap_or_else(|| "—".into()),
                            best_buy_row
                                .map(|r| format!("Best at {}", r.name))
                                .unwrap_or_else(|| "No rates yet".into()),
                            "↓",
                        ))
                        .child(gradient_stat_card(
                            "Best sell rate",
                            best_sell_row
                                .map(|r| theme::format_rate(r.sell))
                                .unwrap_or_else(|| "—".into()),
                            best_sell_row
                                .map(|r| format!("Lowest at {}", r.name))
                                .unwrap_or_else(|| "No rates yet".into()),
                            "↑",
                        )),
                ),
        )
        // Section: buy-rate history chart
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_MD))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(section_title("Buy rate history"))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(theme::SPACE_XS))
                                .child(range_pill(7, history_days, cx))
                                .child(range_pill(30, history_days, cx)),
                        ),
                )
                .child(match history {
                    Some(state) => match &state.result {
                        Some(Ok(points)) => history_chart(points, chart_currency, &state.bank, 220.),
                        Some(Err(err)) => message_box(
                            &format!("Couldn't load history: {err}"),
                            rgb(Palette::DOWN_SOFT),
                            rgb(Palette::DOWN),
                        ),
                        None => skeleton_list(2),
                    },
                    None => message_box(
                        "History loads once rates arrive.",
                        rgb(Palette::SURFACE),
                        rgb(Palette::TEXT_TERTIARY),
                    ),
                }),
        )
        // Section: per-bank rate cards
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_MD))
                .child(section_title("Bank rates"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE_MD))
                        .children(
                            rows.iter()
                                .map(|row| {
                                    let direction =
                                        theme::parse_direction(row.buy_change.as_deref());
                                    let is_best = row.buy == best_buy && rows.len() > 1;
                                    let key = row.key.clone();
                                    let name = row.name.clone();
                                    rate_card(
                                        row,
                                        direction,
                                        is_best,
                                        cx.listener(
                                            move |app, _event: &ClickEvent, _window, cx| {
                                                app.open_drawer(key.clone(), name.clone(), cx);
                                            },
                                        ),
                                    )
                                })
                                .collect::<Vec<_>>(),
                        ),
                ),
        )
        .into_any_element()
}

fn rate_card(
    row: &RateRow,
    direction: theme::Direction,
    is_best: bool,
    on_open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("rate-{}", row.key)))
        .cursor_pointer()
        .on_click(on_open)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_LG))
        .p(px(theme::SPACE_LG))
        .rounded(px(theme::RADIUS_MD))
        .bg(rgb(Palette::SURFACE))
        .border_1()
        .border_color(rgb(Palette::BORDER))
        .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
        .child(
            // Bank identity
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(theme::TEXT_BASE))
                                .text_color(rgb(Palette::TEXT))
                                .child(row.name.clone()),
                        )
                        .when(is_best, |this| {
                            this.child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(theme::RADIUS_SM))
                                    .bg(rgb(Palette::UP_SOFT))
                                    .text_size(px(theme::TEXT_XS))
                                    .text_color(rgb(Palette::UP))
                                    .child("BEST"),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_TERTIARY))
                        .child(
                            row.last_updated
                                .as_deref()
                                .map(theme::short_time)
                                .unwrap_or_else(|| "—".into()),
                        ),
                ),
        )
        // Buy
        .child(
            div()
                .w(px(140.))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child("Buy"),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(theme::TEXT_LG))
                                .text_color(rgb(Palette::TEXT))
                                .child(theme::format_rate(row.buy)),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(direction.color())
                                .child(direction.arrow()),
                        ),
                ),
        )
        // Sell
        .child(
            div()
                .w(px(140.))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child("Sell"),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(theme::TEXT_LG))
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                                .child(theme::format_rate(row.sell)),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(
                                    theme::parse_direction(row.sell_change.as_deref()).color(),
                                )
                                .child(theme::parse_direction(row.sell_change.as_deref()).arrow()),
                        ),
                ),
        )
        .into_any_element()
}

fn banks_view(banks: &[Bank]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .children(
            banks
                .iter()
                .map(|bank| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE_MD))
                        .p(px(theme::SPACE_LG))
                        .rounded(px(theme::RADIUS_MD))
                        .bg(rgb(Palette::SURFACE))
                        .border_1()
                        .border_color(rgb(Palette::BORDER))
                        .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(theme::TEXT_BASE))
                                        .text_color(rgb(Palette::TEXT))
                                        .child(bank.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(theme::TEXT_XS))
                                        .text_color(rgb(Palette::TEXT_TERTIARY))
                                        .child(
                                            bank.last_updated
                                                .as_deref()
                                                .map(theme::short_time)
                                                .unwrap_or_else(|| "—".into()),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(theme::SPACE_SM))
                                .children(bank.rates.iter().map(|rate| {
                                    div()
                                        .px_3()
                                        .py_1()
                                        .rounded(px(theme::RADIUS_SM))
                                        .bg(rgb(Palette::SURFACE_SUNKEN))
                                        .text_size(px(theme::TEXT_XS))
                                        .text_color(rgb(Palette::TEXT_SECONDARY))
                                        .child(format!(
                                            "{}  {} / {}",
                                            rate.currency,
                                            theme::format_rate(rate.buy),
                                            theme::format_rate(rate.sell)
                                        ))
                                })),
                        )
                })
                .collect::<Vec<_>>(),
        )
        .into_any_element()
}

fn averages_view(rows: &[MarketAverage]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .children(
            rows.iter()
                .map(|row| {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::SPACE_LG))
                        .p(px(theme::SPACE_LG))
                        .rounded(px(theme::RADIUS_MD))
                        .bg(rgb(Palette::SURFACE))
                        .border_1()
                        .border_color(rgb(Palette::BORDER))
                        .child(
                            div()
                                .w(px(64.))
                                .text_size(px(theme::TEXT_BASE))
                                .text_color(rgb(Palette::TEXT))
                                .child(row.currency.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(rgb(Palette::UP))
                                .child(format!("buy {}", theme::format_rate(row.avg_buy))),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(rgb(Palette::DOWN))
                                .child(format!("sell {}", theme::format_rate(row.avg_sale))),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                                .child(format!(
                                    "spread {}",
                                    row.avg_spread
                                        .map(|spread| theme::format_rate(spread))
                                        .unwrap_or_else(|| "—".into())
                                )),
                        )
                })
                .collect::<Vec<_>>(),
        )
        .into_any_element()
}

/// Exchange calculator: converts an amount between the selected foreign
/// currency and DOP using the best bank rates for the selected country.
///
/// - Foreign → DOP: banks *buy* your foreign currency, so the best **buy**
///   rate (highest) pays out the most DOP.
/// - DOP → foreign: banks *sell* you foreign currency, so the best **sell**
///   rate (lowest) costs the least DOP.
fn calculator_view(rows: &[RateRow], calc: &CalcContext, cx: &mut Context<DivisasApp>) -> AnyElement {
    let amount_text = calc.amount_text.as_str();
    let direction = calc.direction;
    let selected_currency = calc.currency;
    let selected_country = calc.country;
    let amount: f64 = amount_text.trim().parse().unwrap_or(0.0);

    // Best rate per currency for the current direction.
    let best_for = |currency: &str| -> Option<(&RateRow, f64)> {
        let mut best: Option<(&RateRow, f64)> = None;
        for row in rows.iter().filter(|row| row.currency == currency) {
            let rate = match direction {
                CalcDirection::ForeignToDop => row.buy,
                CalcDirection::DopToForeign => row.sell,
            };
            let better = match &best {
                None => true,
                Some((_, current)) => match direction {
                    CalcDirection::ForeignToDop => rate > *current,
                    CalcDirection::DopToForeign => rate < *current,
                },
            };
            if better {
                best = Some((row, rate));
            }
        }
        best
    };

    let convert = |rate: f64| match direction {
        CalcDirection::ForeignToDop => amount * rate,
        CalcDirection::DopToForeign if rate > 0.0 => amount / rate,
        CalcDirection::DopToForeign => 0.0,
    };

    // Direction toggle segments, labeled with the selected currency.
    let toggle = |target: CalcDirection, label: String, id: &'static str| {
        let selected = direction == target;
        div()
            .id(SharedString::from(id))
            .px_3()
            .py_1()
            .rounded(px(theme::RADIUS_SM))
            .text_size(px(theme::TEXT_SM))
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(rgb(Palette::SURFACE))
                    .text_color(rgb(Palette::TEXT))
            })
            .when(!selected, |this| {
                this.text_color(rgb(Palette::TEXT_SECONDARY))
                    .hover(|style| style.text_color(rgb(Palette::TEXT)))
            })
            .child(label)
            .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                app.calc_direction = target;
                cx.notify();
            }))
    };

    // ---------------------------------------------------------- Hero card
    let hero_result = best_for(selected_currency)
        .map(|(row, rate)| (convert(rate), rate, row.name.clone()))
        .map(|(result, rate, bank)| {
            (
                theme::format_usd(result),
                format!("×{rate:.4}"),
                bank,
            )
        });

    let hero = div()
        .flex()
        .flex_row()
        .gap(px(theme::SPACE_XL))
        .p(px(theme::SPACE_XL))
        .rounded(px(theme::RADIUS_LG))
        .bg(rgb(Palette::SURFACE))
        .border_1()
        .border_color(rgb(Palette::BORDER))
        // Left: amount input + direction toggle.
        .child(
            div()
                .w(px(260.))
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_MD))
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child("AMOUNT"),
                )
                .child(
                    div()
                        .id("calc-amount")
                        .p_2()
                        .rounded(px(theme::RADIUS_SM))
                        .bg(rgb(Palette::SURFACE_SUNKEN))
                        .border_1()
                        .border_color(rgb(Palette::BORDER))
                        .text_size(px(theme::TEXT_LG))
                        .on_click(cx.listener(
                            |app, _: &ClickEvent, window, cx| {
                                window.focus(&app.calc_amount.read(cx).focus_handle(cx));
                            },
                        ))
                        .child(calc.calc_amount.clone()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .p(px(theme::SPACE_XS))
                        .rounded(px(theme::RADIUS_SM))
                        .bg(rgb(Palette::SURFACE_SUNKEN))
                        .child(toggle(
                            CalcDirection::ForeignToDop,
                            format!("{selected_currency} → DOP"),
                            "calc-to-dop",
                        ))
                        .child(toggle(
                            CalcDirection::DopToForeign,
                            format!("DOP → {selected_currency}"),
                            "calc-to-foreign",
                        )),
                ),
        )
        // Right: hero result.
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .text_size(px(theme::TEXT_XS))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child(match direction {
                            CalcDirection::ForeignToDop => "YOU RECEIVE".to_string(),
                            CalcDirection::DopToForeign => "YOU GET".to_string(),
                        }),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_DISPLAY))
                        .text_color(rgb(Palette::UP))
                        .child(
                            hero_result
                                .as_ref()
                                .map(|(result, _, _)| result.clone())
                                .unwrap_or_else(|| "—".into()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(theme::TEXT_SM))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child(
                            hero_result
                                .as_ref()
                                .map(|(_, rate, bank)| {
                                    format!("Best rate {rate} · via {bank}")
                                })
                                .unwrap_or_else(|| {
                                    format!("No {selected_currency} rates in {selected_country}")
                                }),
                        ),
                ),
        );

    // ------------------------------------------------- Per-currency table
    let mut table = div().flex().flex_col().gap(px(theme::SPACE_XS));
    table = table.child(
        div()
            .flex()
            .flex_row()
            .px(px(theme::SPACE_LG))
            .py_1()
            .text_size(px(theme::TEXT_XS))
            .text_color(rgb(Palette::TEXT_TERTIARY))
            .child(
                div()
                    .flex_1()
                    .child(if direction == CalcDirection::ForeignToDop {
                        "SELLING → DOP RECEIVED"
                    } else {
                        "BUYING ← DOP SPENT"
                    }),
            )
            .child(div().w(px(90.)).child("RATE"))
            .child(div().w(px(150.)).child("BEST BANK")),
    );

    for currency in CURRENCIES {
        let Some((row, rate)) = best_for(currency) else {
            continue;
        };
        let result = convert(rate);
        let selected = *currency == selected_currency;
        table = table.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .px(px(theme::SPACE_LG))
                .py(px(theme::SPACE_SM))
                .rounded(px(theme::RADIUS_SM))
                .when(selected, |this| this.bg(rgb(Palette::ACCENT_SOFT)))
                .when(!selected, |this| {
                    this.hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                })
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(theme::TEXT_BASE))
                                .text_color(rgb(if selected {
                                    Palette::ACCENT_TEXT
                                } else {
                                    Palette::TEXT
                                }))
                                .child(format!(
                                "{}",
                                if direction == CalcDirection::ForeignToDop {
                                    format!("{currency} → DOP")
                                } else {
                                    format!("DOP → {currency}")
                                }
                            )),
                        ),
                )
                .child(
                    div()
                        .w(px(120.))
                        .text_size(px(theme::TEXT_BASE))
                        .text_color(rgb(Palette::TEXT))
                        .child(theme::format_usd(result)),
                )
                .child(
                    div()
                        .w(px(90.))
                        .text_size(px(theme::TEXT_SM))
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child(format!("×{rate:.4}")),
                )
                .child(
                    div()
                        .w(px(150.))
                        .text_size(px(theme::TEXT_SM))
                        .text_color(rgb(Palette::TEXT_TERTIARY))
                        .child(row.name.clone()),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_LG))
        .child(hero)
        .child(
            div()
                .p(px(theme::SPACE_MD))
                .rounded(px(theme::RADIUS_MD))
                .bg(rgb(Palette::SURFACE))
                .border_1()
                .border_color(rgb(Palette::BORDER))
                .child(table),
        )
        .into_any_element()
}

fn remittance_view(quotes: &[RemittanceQuote]) -> AnyElement {
    // Sort by effective rate (amount received per USD sent); the best
    // payout wins. Quotes missing `amount_received` sink to the bottom.
    let mut sorted: Vec<&RemittanceQuote> = quotes.iter().collect();
    sorted.sort_by(|a, b| {
        let rate = |q: &RemittanceQuote| q.effective_rate().map(|(rate, _)| rate);
        rate(b)
            .partial_cmp(&rate(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut cards = div().flex().flex_col().gap(px(theme::SPACE_MD));

    // Corridor summary strip: best payout, best rate, typical fee.
    let best = sorted.first().and_then(|q| q.effective_rate());
    let best_fee = sorted.iter().filter_map(|q| q.fee_usd).fold(f64::INFINITY, f64::min);
    let corridor = div()
        .flex()
        .flex_row()
        .gap(px(theme::SPACE_MD))
        .children(
            [
                ("You'd receive", best.map(|(_, amount)| theme::format_usd(amount))),
                ("Best rate", best.map(|(rate, _)| format!("×{rate:.4}"))),
                (
                    "Lowest fee",
                    if best_fee.is_finite() { Some(theme::format_usd(best_fee)) } else { None },
                ),
            ]
            .into_iter()
            .map(|(label, value)| {
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p(px(theme::SPACE_LG))
                    .rounded(px(theme::RADIUS_MD))
                    .bg(rgb(Palette::SURFACE))
                    .border_1()
                    .border_color(rgb(Palette::BORDER))
                    .child(
                        div()
                            .text_size(px(theme::TEXT_XS))
                            .text_color(rgb(Palette::TEXT_SECONDARY))
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TEXT_DISPLAY))
                            .text_color(rgb(Palette::TEXT))
                            .child(value.unwrap_or_else(|| "—".into())),
                    )
            })
            .collect::<Vec<_>>(),
        );
    cards = cards.child(corridor);

    for (index, quote) in sorted.iter().enumerate() {
        let best_quote = index == 0;
        let (effective_rate, amount) = quote
            .effective_rate()
            .map(|(rate, amount)| (Some(rate), Some(amount)))
            .unwrap_or((None, quote.amount_received));

        cards = cards.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_LG))
                .p(px(theme::SPACE_LG))
                .rounded(px(theme::RADIUS_MD))
                .bg(rgb(Palette::SURFACE))
                .border_1()
                .border_color(rgb(if best_quote { Palette::UP } else { Palette::BORDER }))
                .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                .child(
                    // Provider name + delivery speed.
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(theme::SPACE_SM))
                                .child(
                                    div()
                                        .text_size(px(theme::TEXT_BASE))
                                        .text_color(rgb(Palette::TEXT))
                                        .child(quote.name.clone()),
                                )
                                .when(best_quote, |this| {
                                    this.child(
                                        div()
                                            .px_2()
                                            .py(px(2.))
                                            .rounded(px(theme::RADIUS_SM))
                                            .bg(rgb(Palette::UP_SOFT))
                                            .text_size(px(theme::TEXT_XS))
                                            .text_color(rgb(Palette::UP))
                                            .child("BEST PAYOUT"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(rgb(Palette::TEXT_TERTIARY))
                                .child(
                                    quote
                                        .delivery_speed_minutes
                                        .map(|minutes| format!("~{minutes} min delivery"))
                                        .unwrap_or_else(|| quote.key.clone()),
                                ),
                        ),
                )
                .child(
                    // Secondary metrics: rate × fee.
                    div()
                        .w(px(150.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(theme::TEXT_SM))
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                                .child(
                                    effective_rate
                                        .map(|rate| format!("×{rate:.4}"))
                                        .unwrap_or_else(|| "—".into()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_SM))
                                .text_color(rgb(Palette::TEXT_TERTIARY))
                                .child(
                                    quote
                                        .fee_usd
                                        .map(|fee| format!("fee ${fee:.2}"))
                                        .unwrap_or_else(|| "—".into()),
                                ),
                        ),
                )
                .child(
                    // Hero: amount received, sized like a metric tile.
                    div()
                        .w(px(150.))
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(theme::TEXT_LG))
                                .text_color(rgb(if best_quote { Palette::UP } else { Palette::TEXT }))
                                .child(
                                    amount
                                        .map(theme::format_usd)
                                        .unwrap_or_else(|| "—".into()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(theme::TEXT_XS))
                                .text_color(rgb(Palette::TEXT_TERTIARY))
                                .child("received"),
                        ),
                ),
        );
    }

    cards.into_any_element()
}

/// Pulsing placeholder card used while data is loading.
fn skeleton_card(index: usize) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_LG))
        .p(px(theme::SPACE_LG))
        .rounded(px(theme::RADIUS_MD))
        .bg(rgb(Palette::SURFACE))
        .border_1()
        .border_color(rgb(Palette::BORDER))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .w(px(140.))
                        .h(px(12.))
                        .rounded(px(theme::RADIUS_SM))
                        .bg(rgb(Palette::SURFACE_HOVER)),
                )
                .child(
                    div()
                        .w(px(80.))
                        .h(px(9.))
                        .rounded(px(theme::RADIUS_SM))
                        .bg(rgb(Palette::SURFACE_SUNKEN)),
                ),
        )
        .child(
            div()
                .w(px(64.))
                .h(px(18.))
                .rounded(px(theme::RADIUS_SM))
                .bg(rgb(Palette::SURFACE_HOVER)),
        )
        // Gentle opacity pulse; phase offset per card so they shimmer
        // in sequence rather than in lockstep.
        .with_animation(
            ElementId::Name(SharedString::from(format!("skeleton-{index}"))),
            Animation::new(std::time::Duration::from_millis(1400)).repeat(),
            move |card, delta| {
                let phase = (delta + index as f32 * 0.15).fract();
                let pulse = 0.55 + 0.25 * (phase * std::f32::consts::TAU).cos();
                card.opacity(pulse)
            },
        )
        .into_any_element()
}

/// Stack of skeleton cards shown while fetching.
fn skeleton_list(count: usize) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .children((0..count).map(skeleton_card).collect::<Vec<_>>())
        .into_any_element()
}

fn message_box(text: &str, bg: gpui::Rgba, fg: gpui::Rgba) -> AnyElement {
    div()
        .p(px(theme::SPACE_LG))
        .rounded(px(theme::RADIUS_MD))
        .bg(bg)
        .text_color(fg)
        .text_size(px(theme::TEXT_BASE))
        .child(text.to_string())
        .into_any_element()
}


// ----------------------------------------------------------------------
// Entry point
// ----------------------------------------------------------------------

fn main() {
    Application::new().run(|cx: &mut App| {
        text_input::bind_text_input_keys(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-r", Refresh, Some("divisas")),
            KeyBinding::new("cmd-q", Quit, Some("divisas")),
            KeyBinding::new("cmd-1", SwitchTab1, Some("divisas")),
            KeyBinding::new("cmd-2", SwitchTab2, Some("divisas")),
            KeyBinding::new("cmd-3", SwitchTab3, Some("divisas")),
            KeyBinding::new("cmd-4", SwitchTab4, Some("divisas")),
            KeyBinding::new("cmd-5", SwitchTab5, Some("divisas")),
            KeyBinding::new("cmd-6", SwitchTab6, Some("divisas")),
            KeyBinding::new("cmd-shift-s", SignOut, Some("divisas")),
        ]);

        let web_api_key = std::env::var("DIVISAS_WEB_API_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .unwrap_or(divisas_api::DEFAULT_WEB_API_KEY.into());
        let base_url = std::env::var("DIVISAS_API_BASE_URL")
            .unwrap_or(divisas_api::DEFAULT_BASE_URL.into());

        // Prefer an env-provided key, then the stored session; otherwise
        // restore a guest session silently or show login.
        let stored_key = std::env::var("DIVISAS_API_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .or_else(session::load_api_key);
        let last_method = session::load_auth_method();

        let app = match stored_key {
            Some(api_key) => cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                let search = cx.new(|cx| TextInput::new("", "Search…", false, cx));
                cx.observe(&search, |_, _, cx| cx.notify()).detach();
                let calc_amount = cx.new(|cx| TextInput::new("100", "Amount", false, cx));
                cx.observe(&calc_amount, |_, _, cx| cx.notify()).detach();
                let mut app = DivisasApp {
                    client: DivisasClient::with_base_url(api_key, base_url.clone()),
                    web_api_key: web_api_key.clone(),
                    base_url,
                    auth_method: AuthMethod::Email,
                    focus_handle,
                    screen: Screen::Main,
                    tab: Tab::Rates,
                    country: 0,
                    currency: 0,
                    data: Data::Empty,
                    status: None,
                    drawer: None,
                    reauth_attempts: 0,
                    search,
                    calc_amount,
                    calc_direction: CalcDirection::ForeignToDop,
                    history: None,
                    history_days: 30,
                    about_open: [false; 3],
                };
                app.fetch(cx);
                app
            }),
            None => cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                let search = cx.new(|cx| TextInput::new("", "Search…", false, cx));
                cx.observe(&search, |_, _, cx| cx.notify()).detach();
                let calc_amount = cx.new(|cx| TextInput::new("100", "Amount", false, cx));
                cx.observe(&calc_amount, |_, _, cx| cx.notify()).detach();
                let mut app = DivisasApp {
                    client: DivisasClient::with_base_url("", base_url.clone()),
                    web_api_key: web_api_key.clone(),
                    base_url,
                    auth_method: AuthMethod::Email,
                    focus_handle,
                    screen: Screen::Login {
                        email: cx.new(|cx| TextInput::new("", "email@example.com", false, cx)),
                        password: cx.new(|cx| TextInput::new("", "password", true, cx)),
                        error: None,
                        busy: false,
                    },
                    tab: Tab::Rates,
                    country: 0,
                    currency: 0,
                    data: Data::Empty,
                    status: None,
                    drawer: None,
                    reauth_attempts: 0,
                    search,
                    calc_amount,
                    calc_direction: CalcDirection::ForeignToDop,
                    history: None,
                    history_days: 30,
                    about_open: [false; 3],
                };
                // Guest sessions restore silently: mint a fresh key without
                // showing the login form. Email sessions can't be silent
                // (we never store passwords), so show the login screen.
                if last_method.as_deref() != Some("email") {
                    app.sign_in_anonymous(cx);
                }
                app
            }),
        };

        let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Divisas Desktop".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, _cx| app.clone(),
        )
        .unwrap();

        cx.activate(true);
    });
}
