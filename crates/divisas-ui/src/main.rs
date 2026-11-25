//! Divisas Desktop — GPUI client for the divisas-cloud public API.
//!
//! Set `DIVISAS_API_KEY` in the environment before launching (all endpoints
//! except `apiHealth` require it). Optionally override the deployed base URL
//! with `DIVISAS_API_BASE_URL`.

use gpui::{
    App, AppContext, Application, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable,
    KeyBinding, Render, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
    actions, div, prelude::*, px, rgb, size, white,
};

use divisas_api::{
    ApiError, Bank, COUNTRIES, CURRENCIES, DivisasClient, MarketAverage, RateRow, RemittanceQuote,
    flatten_averages,
};

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

struct DivisasApp {
    client: DivisasClient,
    focus_handle: FocusHandle,
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
                Tab::Official => client
                    .official(None)
                    .map(|r| Data::Rates(
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
        .detach();    }

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
            .on_action(|_: &Quit, cx| cx.quit())
            .bg(rgb(0x0B1220))
            .text_color(rgb(0xE5E7EB))
            .size_full()
            .flex()
            .flex_col()
            .child(
                // Title bar
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
                // Tab bar
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .px_4()
                    .bg(rgb(0x111827))
                    .children(tab_bar),
            )
            .child(
                // Filter chips
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

fn message_box(text: &str, bg: u32, fg: u32) -> AnyElement {
    div()
        .m_4()
        .p_3()
        .rounded(px(8.))
        .bg(rgb(bg))
        .text_color(rgb(fg))
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
                .child(*label)
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
        let api_key = std::env::var("DIVISAS_API_KEY").unwrap_or_default();
        let base_url =
            std::env::var("DIVISAS_API_BASE_URL").unwrap_or(divisas_api::DEFAULT_BASE_URL.into());
        let client = DivisasClient::with_base_url(api_key, base_url);

        cx.bind_keys([
            KeyBinding::new("cmd-r", Refresh, Some("divisas")),
            KeyBinding::new("cmd-q", Quit, Some("divisas")),
        ]);

        let app = cx.new(|cx| {
            let focus_handle = cx.focus_handle();
            DivisasApp {
                client,
                focus_handle,
                tab: Tab::Rates,
                country: 0,
                currency: 0,
                data: Data::Empty,
                status: None,
            }
        });

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

        app.update(cx, |app, cx| app.fetch(cx)).ok();
        cx.activate(true);
    });
}
