//! Divisas Desktop — GPUI client for the divisas-cloud API.
//!
//! On first launch a login screen collects email + password, signs in against
//! Firebase Identity Toolkit (needs `DIVISAS_WEB_API_KEY`), mints an API key
//! via `POST /v1/keys`, and stores it in the OS keychain. Later launches load
//! the key straight from the keychain (or `DIVISAS_API_KEY`).

mod keychain;
mod text_input;
mod theme;

use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Application, Bounds, ClickEvent, Context,
    ElementId, Entity, FocusHandle, Focusable, KeyBinding, Render, SharedString, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size, white,
};
use gpui::Rgba;

use divisas_api::{ApiKeyIssued, ApiError, Bank, COUNTRIES, CURRENCIES, DivisasClient, HistoryPoint, MarketAverage, RateRow, RemittanceQuote, flatten_averages};
use text_input::TextInput;
use theme::Palette;
use theme::{DOWN, DOWN_SOFT, TEXT_PRIMARY, UP};

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
    drawer: Option<Drawer>,
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

        // Sidebar navigation
        let nav_items = Tab::ALL
            .map(|candidate| {
                let selected = candidate == tab;
                div()
                    .id(SharedString::from(format!("nav-{}", candidate.label())))
                    .px_3()
                    .py_2()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .text_size(px(theme::TEXT_BASE))
                    .when(selected, |this| {
                        this.bg(rgb(Palette::ACCENT_SOFT)).text_color(rgb(Palette::ACCENT_TEXT))
                    })
                    .when(!selected, |this| {
                        this.text_color(rgb(Palette::TEXT_SECONDARY))
                            .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                    })
                    .child(candidate.label())
                    .on_click(cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                        app.set_tab(candidate, cx);
                    }))
            })
            .into_iter()
            .collect::<Vec<_>>();

        // Segmented country switcher
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
                        this.bg(rgb(Palette::ACCENT_SOFT)).text_color(rgb(Palette::ACCENT_TEXT))
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
                        this.bg(rgb(Palette::ACCENT_SOFT)).text_color(rgb(Palette::ACCENT_TEXT))
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
            .on_action(|_: &Quit, _window, cx| cx.quit())
            .bg(rgb(Palette::ROOT))
            .text_color(rgb(Palette::TEXT))
            .size_full()
            .flex()
            .flex_row()
            // ------------------------------------------------ Sidebar
            .child(
                div()
                    .w(px(220.))
                    .flex()
                    .flex_col()
                    .bg(rgb(Palette::SIDEBAR))
                    .border_r_1()
                    .border_color(rgb(Palette::BORDER))
                    .p(px(theme::SPACE_LG))
                    .child(
                        div()
                            .px_3()
                            .pb(px(theme::SPACE_XL))
                            .child(
                                div()
                                    .text_size(px(theme::TEXT_LG))
                                    .text_color(rgb(Palette::TEXT))
                                    .child("Divisas"),
                            )
                            .child(
                                div()
                                    .text_size(px(theme::TEXT_XS))
                                    .text_color(rgb(Palette::TEXT_TERTIARY))
                                    .child("Exchange rates · divisas-cloud"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE_XS))
                            .children(nav_items),
                    )
                    .child(
                        div()
                            .flex_1()
                            .items_end()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .px_3()
                                    .text_size(px(theme::TEXT_XS))
                                    .text_color(rgb(Palette::TEXT_TERTIARY))
                                    .child("⌘R refresh · ⌘Q quit"),
                            ),
                    ),
            )
            // ------------------------------------------------ Main column
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        // Header: title + segmented switchers
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE_LG))
                            .px(px(theme::SPACE_2XL))
                            .pt(px(theme::SPACE_XL))
                            .pb(px(theme::SPACE_LG))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(theme::TEXT_XL))
                                            .text_color(rgb(Palette::TEXT))
                                            .child(tab.label()),
                                    )
                                    .child(
                                        div()
                                            .id("refresh")
                                            .px_4()
                                            .py_2()
                                            .rounded(px(theme::RADIUS_SM))
                                            .bg(rgb(Palette::SURFACE))
                                            .border_1()
                                            .border_color(rgb(Palette::BORDER))
                                            .text_size(px(theme::TEXT_SM))
                                            .text_color(rgb(Palette::TEXT_SECONDARY))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
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
                                    .items_center()
                                    .gap(px(theme::SPACE_LG))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_1()
                                            .p(px(theme::SPACE_XS))
                                            .rounded(px(theme::RADIUS_SM))
                                            .bg(rgb(Palette::SURFACE_SUNKEN))
                                            .children(country_segments),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_1()
                                            .p(px(theme::SPACE_XS))
                                            .rounded(px(theme::RADIUS_SM))
                                            .bg(rgb(Palette::SURFACE_SUNKEN))
                                            .children(currency_segments),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("content")
                            .flex_1()
                            .overflow_y_scroll()
                            .px(px(theme::SPACE_2XL))
                            .pb(px(theme::SPACE_XL))
                            .child(
                                div()
                                    .max_w(px(880.))
                                    .child(render_body(&self.data, cx)),
                            ),
                    )
                    .child(
                        div()
                            .px(px(theme::SPACE_2XL))
                            .py(px(theme::SPACE_MD))
                            .border_t_1()
                            .border_color(rgb(Palette::BORDER))
                            .text_size(px(theme::TEXT_XS))
                            .text_color(rgb(Palette::TEXT_TERTIARY))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(theme::SPACE_SM))
                            // Stale-data indicator: green "fresh" dot, amber when > 30 min.
                            .children(updated_label(&self.data).map(|(stale, label)| {
                                let dot = if stale { Palette::WARN } else { Palette::UP };
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(theme::SPACE_XS))
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

fn render_body(data: &Data, cx: &mut Context<DivisasApp>) -> AnyElement {
    match data {
        Data::Empty => message("No data. Press Refresh."),
        Data::Loading => skeleton_list(6),
        Data::Error(message) => message_box(message, DOWN_SOFT, DOWN),
        Data::Rates(rows) => rates_view(rows, cx),
        Data::Banks(banks) => banks_view(banks),
        Data::Averages(rows) => averages_view(rows),
        Data::Remittance(quotes) => remittance_view(quotes),
    }
}

/// Metric strip shown above the rates grid.
fn metric_tile(label: &str, value: String, color: Rgba) -> AnyElement {
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
                .text_size(px(theme::TEXT_LG))
                .text_color(color)
                .child(value),
        )
        .into_any_element()
}

fn rates_view(rows: &[RateRow], cx: &mut Context<DivisasApp>) -> AnyElement {
    let best_buy = rows.iter().map(|r| r.buy).fold(f64::NEG_INFINITY, f64::max);
    let best_sell = rows.iter().map(|r| r.sell).fold(f64::INFINITY, f64::min);
    let avg_buy = if rows.is_empty() {
        0.
    } else {
        rows.iter().map(|r| r.buy).sum::<f64>() / rows.len() as f64
    };

    div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_LG))
        .child(
            // Metric strip
            div()
                .flex()
                .flex_row()
                .gap(px(theme::SPACE_MD))
                .child(metric_tile("Best buy", theme::format_rate(best_buy), UP))
                .child(metric_tile("Best sell", theme::format_rate(best_sell), DOWN))
                .child(metric_tile("Market avg", theme::format_rate(avg_buy), TEXT_PRIMARY))
                .child(metric_tile(
                    "Banks",
                    rows.len().to_string(),
                    TEXT_PRIMARY,
                )),
        )
        .child(
            // Rate cards grid
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_MD))
                .children(
                    rows.iter()
                        .map(|row| {
                            let direction = theme::parse_direction(row.buy_change.as_deref());
                            let is_best = row.buy == best_buy && rows.len() > 1;
                            let key = row.key.clone();
                            let name = row.name.clone();
                            rate_card(
                                row,
                                direction,
                                is_best,
                                cx.listener(move |app, _event: &ClickEvent, _window, cx| {
                                    app.open_drawer(key.clone(), name.clone(), cx);
                                }),
                            )
                        })
                        .collect::<Vec<_>>(),
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

fn remittance_view(quotes: &[RemittanceQuote]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_MD))
        .children(
            quotes
                .iter()
                .enumerate()
                .map(|(index, quote)| {
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
                        .hover(|style| style.bg(rgb(Palette::SURFACE_HOVER)))
                        .child(
                            div()
                                .w(px(32.))
                                .text_size(px(theme::TEXT_LG))
                                .text_color(rgb(Palette::TEXT_TERTIARY))
                                .child(format!("{}", index + 1)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(theme::TEXT_BASE))
                                .text_color(rgb(Palette::TEXT))
                                .child(quote.name.clone()),
                        )
                        .child(
                            div()
                                .w(px(110.))
                                .text_size(px(theme::TEXT_SM))
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                                .child(
                                    quote
                                        .exchange_rate
                                        .map(|rate| format!("×{:.4}", rate))
                                        .unwrap_or_else(|| "—".into()),
                                ),
                        )
                        .child(
                            div()
                                .w(px(90.))
                                .text_size(px(theme::TEXT_SM))
                                .text_color(rgb(Palette::TEXT_SECONDARY))
                                .child(
                                    quote
                                        .fee_usd
                                        .map(|fee| format!("${fee:.2}"))
                                        .unwrap_or_else(|| "—".into()),
                                ),
                        )
                        .child(
                            div()
                                .w(px(120.))
                                .text_size(px(theme::TEXT_BASE))
                                .text_color(rgb(Palette::UP))
                                .child(
                                    quote
                                        .amount_received
                                        .map(|amount| format!("${amount:.2}"))
                                        .unwrap_or_else(|| "—".into()),
                                ),
                        )
                })
                .collect::<Vec<_>>(),
        )
        .into_any_element()
}

fn message(text: &str) -> AnyElement {
    div()
        .p(px(theme::SPACE_2XL))
        .text_size(px(theme::TEXT_BASE))
        .text_color(rgb(Palette::TEXT_TERTIARY))
        .child(text.to_string())
        .into_any_element()
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
                    drawer: None,
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
                    drawer: None,
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
