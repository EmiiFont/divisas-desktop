//! Divisas Desktop — GPUI client for the divisas-cloud API.
//!
//! On first launch a login screen collects email + password, signs in against
//! Firebase Identity Toolkit (needs `DIVISAS_WEB_API_KEY`), mints an API key
//! via `POST /v1/keys`, and stores it in the OS keychain. Later launches load
//! the key straight from the keychain (or `DIVISAS_API_KEY`).

mod keychain;
mod text_input;

use gpui::{
    AnyElement, App, AppContext, Application, Bounds, ClickEvent, Context, Entity, FocusHandle,
    Focusable, KeyBinding, Render, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, div, prelude::*, px, rgb, size, white,
};

use divisas_api::{ApiKeyIssued, ApiError, Bank, COUNTRIES, CURRENCIES, DivisasClient, MarketAverage, RateRow, RemittanceQuote, flatten_averages};
use text_input::TextInput;

// ----------------------------------------------------------------------
// Actions
// ----------------------------------------------------------------------

actions!(divisas, [Refresh, Quit]);

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
}

impl Tab {
    fn label(&self) -> &'static str {
        match self {
            Tab::Rates => "Rates",
            Tab::Banks => "Banks",
            Tab::Official => "Official",
            Tab::Averages => "Averages",
            Tab::Remittance => "Remittance",
        }
    }

    const ALL: [Tab; 5] = [
        Tab::Rates,
        Tab::Banks,
        Tab::Official,
        Tab::Averages,
        Tab::Remittance,
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

struct DivisasApp {
    client: DivisasClient,
    web_api_key: String,
    focus_handle: FocusHandle,
    screen: Screen,
    tab: Tab,
    country: usize,
    currency: usize,
    data: Data,
    status: Option<SharedString>,
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
            };
            match result {
                Ok(data) => Ok(data),
                Err(ApiError::Api { status, code, message }) => Err(format!(
                    "HTTP {status} — {code}: {message}"
                )),
                Err(err) => Err(err.to_string()),
            }
        });

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |app, cx| {
                app.status = None;
                app.data = match result {
                    Ok(data) => data,
                    Err(message) => Data::Error(message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let Screen::Login { error, busy, .. } = &mut self.screen else {
            return;
        };
        *busy = false;
        match result {
            Ok(issued) => {
                if let Err(err) = keychain::save_api_key(&issued.api_key) {
                    *error = Some(format!("Signed in, but could not store key: {err}"));
                } else {
                    self.client = DivisasClient::with_base_url(
                        issued.api_key,
                        divisas_api::DEFAULT_BASE_URL,
                    );
                    self.screen = Screen::Main;
                    self.fetch(cx);
                }
            }
            Err(ApiError::Api { status, code, message }) => {
                *error = Some(format!("HTTP {status} — {code}: {message}"));
            }
            Err(err) => {
                *error = Some(err.to_string());
            }
        }
        cx.notify();
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

        let country_chips = COUNTRIES
            .iter()
            .enumerate()
            .map(|(index, code)| {
                let selected = index == self.country;
                div()
                    .id(SharedString::from(format!("country-{code}")))
                    .px_3()
                    .py_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(selected, |this| this.bg(rgb(0x2563EB)).text_color(white()))
                    .when(!selected, |this| {
                        this.bg(rgb(0x1F2937)).text_color(rgb(0x9CA3AF))
                    })
                    .child(*code)
                    .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                        app.set_country(index, cx);
                    }))
            })
            .collect::<Vec<_>>();

        let currency_chips = CURRENCIES
            .iter()
            .enumerate()
            .map(|(index, code)| {
                let selected = index == self.currency;
                div()
                    .id(SharedString::from(format!("currency-{code}")))
                    .px_3()
                    .py_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(selected, |this| this.bg(rgb(0x2563EB)).text_color(white()))
                    .when(!selected, |this| {
                        this.bg(rgb(0x1F2937)).text_color(rgb(0x9CA3AF))
                    })
                    .child(*code)
                    .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                        app.set_currency(index, cx);
                    }))
            })
            .collect::<Vec<_>>();

        let tab_bar = Tab::ALL
            .map(|candidate| {
                let selected = candidate == tab;
                div()
                    .id(SharedString::from(format!("tab-{}", candidate.label())))
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(selected, |this| {
                        this.text_color(white())
                            .border_b_2()
                            .border_color(rgb(0x2563EB))
                    })
                    .when(!selected, |this| this.text_color(rgb(0x9CA3AF)))
                    .child(candidate.label())
                    .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                        app.set_tab(candidate, cx);
                    }))
            })
            .into_iter()
            .collect::<Vec<_>>();

        div()
            .track_focus(&self.focus_handle)
            .key_context("divisas")
            .on_action(cx.listener(|app, _: &Refresh, _window, cx| app.fetch(cx)))
            .on_action(|_: &Quit, _window, cx| cx.quit())
            .bg(rgb(0x0B1220))
            .text_color(rgb(0xE5E7EB))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x111827))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .text_color(white())
                                    .child("Divisas Desktop"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x6B7280))
                                    .child(format!("{} · {}", country, currency)),
                            ),
                    )
                    .child(
                        div()
                            .id("refresh")
                            .px_3()
                            .py_1()
                            .rounded(px(6.))
                            .bg(rgb(0x1F2937))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .child("Refresh")
                            .on_click(cx.listener(|app, _event: &ClickEvent, _window, cx| {
                                app.fetch(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .px_4()
                    .bg(rgb(0x111827))
                    .children(tab_bar),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x6B7280))
                                    .child("Country"),
                            )
                            .children(country_chips),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(0x6B7280))
                                    .child("Currency"),
                            )
                            .children(currency_chips),
                    ),
            )
            .child(
                div()
                    .id("content")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_4()
                    .child(render_body(&self.data)),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x111827))
                    .text_size(px(11.))
                    .text_color(rgb(0x6B7280))
                    .child(self.status.clone().unwrap_or_else(|| {
                        "divisas-cloud · GET /v1/rates/latest, /v1/banks, /v1/official, /v1/averages, /v1/remittance/quotes".into()
                    })),
            )
    }
}

fn render_body(data: &Data) -> AnyElement {
    match data {
        Data::Empty => message("No data. Press Refresh."),
        Data::Loading => message("Loading…"),
        Data::Error(message) => message_box(message, rgb(0x7F1D1D), rgb(0xFCA5A5)),
        Data::Rates(rows) => table(
            &["Bank", "Currency", "Buy", "Sell"],
            rows.iter()
                .map(|row| {
                    vec![
                        row.name.clone(),
                        row.currency.clone(),
                        format!("{:.4}", row.buy),
                        format!("{:.4}", row.sell),
                    ]
                })
                .collect(),
        ),
        Data::Banks(banks) => table(
            &["Bank", "Type", "Rates", "Last updated"],
            banks
                .iter()
                .map(|bank| {
                    vec![
                        bank.name.clone(),
                        bank.institution_type.clone().unwrap_or_default(),
                        bank.rates
                            .iter()
                            .map(|rate| format!("{} {:.4}/{:.4}", rate.currency, rate.buy, rate.sell))
                            .collect::<Vec<_>>()
                            .join(" · "),
                        bank.last_updated.clone().unwrap_or_default(),
                    ]
                })
                .collect(),
        ),
        Data::Averages(rows) => table(
            &["Country", "Currency", "Avg buy", "Avg sale", "Spread"],
            rows.iter()
                .map(|row| {
                    vec![
                        row.country.clone(),
                        row.currency.clone(),
                        format!("{:.4}", row.avg_buy),
                        format!("{:.4}", row.avg_sale),
                        row.avg_spread
                            .map(|spread| format!("{spread:.4}"))
                            .unwrap_or_else(|| "—".into()),
                    ]
                })
                .collect(),
        ),
        Data::Remittance(quotes) => table(
            &["Provider", "Rate", "Fee USD", "Receive"],
            quotes
                .iter()
                .map(|quote| {
                    vec![
                        quote.name.clone(),
                        quote
                            .exchange_rate
                            .map(|rate| format!("{rate:.4}"))
                            .unwrap_or_else(|| "—".into()),
                        quote
                            .fee_usd
                            .map(|fee| format!("{fee:.2}"))
                            .unwrap_or_else(|| "—".into()),
                        quote
                            .amount_received
                            .map(|amount| format!("{amount:.2}"))
                            .unwrap_or_else(|| "—".into()),
                    ]
                })
                .collect(),
        ),
    }
}

fn message(text: &str) -> AnyElement {
    div()
        .p_4()
        .text_size(px(13.))
        .text_color(rgb(0x6B7280))
        .child(text.to_string())
        .into_any_element()
}

fn message_box(text: &str, bg: gpui::Rgba, fg: gpui::Rgba) -> AnyElement {
    div()
        .m_4()
        .p_3()
        .rounded(px(8.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(13.))
        .child(text.to_string())
        .into_any_element()
}

fn table(header: &[&str], rows: Vec<Vec<String>>) -> AnyElement {
    let header_cells = header
        .iter()
        .map(|label| {
            div()
                .flex_1()
                .px_3()
                .py_2()
                .text_size(px(11.))
                .text_color(rgb(0x9CA3AF))
                .child(label.to_string())
        })
        .collect::<Vec<_>>();

    let body = rows
        .into_iter()
        .map(|cells| {
            div()
                .flex()
                .flex_row()
                .w_full()
                .rounded(px(6.))
                .bg(rgb(0x111827))
                .mb_1()
                .children(cells.into_iter().map(|cell| {
                    div()
                        .flex_1()
                        .px_3()
                        .py_2()
                        .text_size(px(12.))
                        .child(cell)
                }))
        })
        .collect::<Vec<_>>();

    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .w_full()
                .border_b_1()
                .border_color(rgb(0x1F2937))
                .children(header_cells),
        )
        .children(body)
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
        ]);

        let web_api_key = std::env::var("DIVISAS_WEB_API_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .unwrap_or(divisas_api::DEFAULT_WEB_API_KEY.into());
        let base_url = std::env::var("DIVISAS_API_BASE_URL")
            .unwrap_or(divisas_api::DEFAULT_BASE_URL.into());

        // Prefer an env-provided key, then the keychain; otherwise show login.
        let stored_key = std::env::var("DIVISAS_API_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .or_else(keychain::load_api_key);

        let app = match stored_key {
            Some(api_key) => cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                let mut app = DivisasApp {
                    client: DivisasClient::with_base_url(api_key, base_url),
                    web_api_key,
                    focus_handle,
                    screen: Screen::Main,
                    tab: Tab::Rates,
                    country: 0,
                    currency: 0,
                    data: Data::Empty,
                    status: None,
                };
                app.fetch(cx);
                app
            }),            None => cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                let email = cx.new(|cx| TextInput::new("", "email@example.com", false, cx));
                let password = cx.new(|cx| TextInput::new("", "password", true, cx));
                DivisasApp {
                    client: DivisasClient::with_base_url("", base_url),
                    web_api_key,
                    focus_handle,
                    screen: Screen::Login {
                        email,
                        password,
                        error: None,
                        busy: false,
                    },
                    tab: Tab::Rates,
                    country: 0,
                    currency: 0,
                    data: Data::Empty,
                    status: None,
                }
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
