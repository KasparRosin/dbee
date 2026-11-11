//! Every control of the design's Components page in every state, with no database, for looking
//! at them against the page's light and dark frames (`examples/components.rs`): one section per
//! page component, in the page's order, each headed with the page's caption.

use std::sync::LazyLock;
use std::time::Duration;

use egui::{Align, Align2, Color32, CornerRadius, Label, Layout, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};

use dbee_config::ConnectionColor;

use super::Look;
use super::button::{BarButton, BoxButton, IconButton, TextButton};
use super::checkbox::Checkbox;
use super::chip::Chip;
use super::dialog::{Button, Dialog};
use super::dropdown::Dropdown;
use super::icon::Icon;
use super::input::{Field, Input};
use super::keycap::Keycap;
use super::menu::{self, BarTitle, Item};
use super::results::{ResultBar, ResultTab, RowCount, RunRow, RunState};
use super::segmented::{Segment, Segmented};
use super::sidebar::{self, Count};
use super::status::{self, Activity, Connection, Facts};
use super::swatch::{COLOURS, Swatch};
use super::tab::{self, NewTab, Tab};
use super::table_bars::{self, Loading, PagingBar, TableBar, View};
use super::toast::Toast;
use super::tooltip;
use crate::theme::size::{BAR_H, KEYCAP_GAP, MENU_PAD, RADIUS_CONTROL, SLIM_BAR_H, SPACE_8, SPACE_12, SPACE_16, SPACE_24, SPACE_32, TAB_H};
use crate::theme::{Text, Tokens, Words, tokens};

/// The page's frames: 20 between sections.
const BETWEEN: f32 = 20.0;
/// Between the controls of a row of buttons.
const ROW_GAP: f32 = 20.0;
/// The Field and input frame's inputs and fields, and the Dropdown frame's dropdowns.
const INPUT_W: f32 = 160.0;
/// The Segmented frame's small boxes, and the width of a small segment shown alone.
const SEGMENTED_W: f32 = 386.0;
const SMALL_SEGMENT_W: f32 = 77.0;
/// The Icons frame: 20 under its caption; cells of 120 by 40, nine to a row, 8 apart and 20
/// between rows. In a cell the icon's box ends 18 down, centred, and its name starts 24 down.
const ICONS_UNDER_CAPTION: f32 = 20.0;
const ICON_CELL: Vec2 = vec2(120.0, 40.0);
const ICONS_PER_ROW: usize = 9;
const ICON_CELL_GAP: Vec2 = vec2(8.0, 20.0);
const ICON_BOTTOM: f32 = 18.0;
const ICON_NAME_TOP: f32 = 24.0;
/// The check box's mark is shown on an `ink` tile, as in a checkbox.
const ICON_TILE: f32 = 14.0;
/// The Menus frame: the items' widths of its menus (Edit, View, Theme), its scene with the menu
/// bar, and the Theme submenu's overlap of the View menu.
const MENU_W: f32 = 230.0;
const VIEW_MENU_W: f32 = 190.0;
const THEME_MENU_W: f32 = 130.0;
const SCENE: Vec2 = vec2(560.0, 190.0);
const MENU_BAR_W: f32 = 240.0;
const SUBMENU_OVERLAP: f32 = 4.0;
/// The Tabs and bars frame's tab strip and menu bars.
const STRIP_W: f32 = 840.0;
/// The filter bar frame's column, operator and value, as wide as the page draws them.
const FILTER_COLUMN_W: f32 = 140.0;
const FILTER_OP_W: f32 = 160.0;
const FILTER_VALUE_W: f32 = 140.0;
/// The sidebar header frame's filters and counts, as wide as the page draws them.
const SIDEBAR_W: f32 = 259.0;
/// The status bar frames: bars this wide, this far apart.
const STATUS_W: f32 = 1100.0;
const STATUS_APART: f32 = 10.0;
/// The status bars' addresses, as the page writes them, and a long path.
static PG14: LazyLock<status::Address> = LazyLock::new(|| status::Address::Server("postgres@pg14:5432/dbee".into()));
static TUNNEL: LazyLock<status::Address> = LazyLock::new(|| status::Address::Server("postgres@db.internal:5432/dbee via bastion.example.com".into()));
static SHOP_FILE: LazyLock<status::Address> = LazyLock::new(|| status::Address::File("~/data/shop.sqlite".into()));
static LONG_PATH: LazyLock<status::Address> =
    LazyLock::new(|| status::Address::File("~/Projects/clients/northwind-traders/archive/2026/exports/nightly/shop.sqlite".into()));
/// A bar too narrow for the long path.
const LONG_PATH_W: f32 = 560.0;
/// The Dialogs frame's scrim scene: a window of this size, its query text this far in, the dialog
/// this far in.
const DIALOG_SCENE: Vec2 = vec2(640.0, 240.0);
const BEHIND_AT: Vec2 = vec2(24.0, 20.0);
const DIALOG_AT: Vec2 = vec2(100.0, 40.0);
const BEHIND: &str = "select o.id, c.name as customer, o.total\nfrom shop.orders o\njoin shop.customers c on c.id = o.customer_id\nwhere o.total > 100\norder by o.id desc\nlimit 50;";

/// The icons in the order of the page's Icons frame.
const ICONS: [Icon; 32] = [
    Icon::ChevronDown,
    Icon::KeyForeign,
    Icon::KeyPrimary,
    Icon::MaterializedView,
    Icon::View,
    Icon::Table,
    Icon::Schema,
    Icon::Link,
    Icon::NoColour,
    Icon::Search,
    Icon::Wrap,
    Icon::Copy,
    Icon::CheckBox,
    Icon::Check,
    Icon::Last,
    Icon::Next,
    Icon::Previous,
    Icon::First,
    Icon::Code,
    Icon::Import,
    Icon::Export,
    Icon::Panel,
    Icon::TabQuery,
    Icon::TabTable,
    Icon::Refresh,
    Icon::Close,
    Icon::Minus,
    Icon::Plus,
    Icon::Key,
    Icon::SortDescending,
    Icon::SortAscending,
    Icon::ChevronRight,
];

/// Sets dbee's fonts and theme on `ctx`, drawing in `theme`. The fonts are there from the next
/// frame on.
pub fn prepare(ctx: &egui::Context, theme: egui::Theme) {
    crate::theme::apply(ctx);
    ctx.set_theme(theme);
}

/// Every control in every state, one section per page component in the page's order.
pub fn show(ui: &mut Ui) {
    egui::Frame::NONE.inner_margin(SPACE_24).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        section(ui, "icon/, today's drawings in their usual colour", ICONS_UNDER_CAPTION, icons);
        buttons(ui);
        inputs(ui);
        choices(ui);
        section(ui, "keycap: default, disabled", SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = KEYCAP_GAP;
            for key in ["Ctrl", "Shift", "Z"] {
                ui.add(Keycap::new(key));
            }
            // The page's 24 between the groups is an item of the row, with the gap either side.
            ui.add_space(SPACE_24 + KEYCAP_GAP);
            for key in ["Ctrl", "Z"] {
                ui.add_enabled(false, Keycap::new(key));
            }
        });
        section(ui, "chip: next, available", SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_16;
            ui.add(Chip::next("Upcoming"));
            ui.add(Chip::available("Available"));
        });
        menus(ui);
        tooltips_and_toasts(ui);
        dialogs(ui);
        tabs(ui);
        bars(ui);
    });
}

/// The menu bar, the query tab's bars and the status bars, as the page's Tabs and bars frame and
/// its Status bar frame show them.
fn bars(ui: &mut Ui) {
    section(ui, "menu bar, 30 tall, from menubar/title; then with View open", SPACE_8, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SPACE_8;
            for open in [None, Some("View")] {
                let (rect, _) = ui.allocate_exact_size(vec2(STRIP_W, SLIM_BAR_H), Sense::hover());
                let titles = ["File", "Edit", "View", "Query"].map(|w| (w, Some(w) == open));
                menu_bar(ui, rect.min, STRIP_W, &titles);
            }
        });
    });
    results(ui);
    table_tab_bars(ui);
    sidebar_header(ui);
    let shop = |colour| Connection { name: "Shop live", colour, own: true };
    let stack = |ui: &mut Ui, rows: Vec<(Connection<'static>, Facts<'static>, Option<Activity>)>| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = STATUS_APART;
            for (connection, facts, activity) in &rows {
                ui.allocate_ui(vec2(STATUS_W, SLIM_BAR_H), |ui| status::bar(ui, connection, facts, activity.as_ref(), |_| {}));
            }
        });
    };
    let pg14 = Some(&*PG14);
    section(ui, "status bar: the connection colour as a 2 px top edge and the name block; no colour keeps a quiet footer", SPACE_8, |ui| {
        let mut colours: Vec<ConnectionColor> = COLOURS.into_iter().filter(|c| *c != ConnectionColor::None).collect();
        colours.push(ConnectionColor::None);
        stack(ui, colours.into_iter().map(|c| (shop(c), Facts { address: pg14, ..Facts::default() }, None)).collect());
    });
    section(
        ui,
        "new: address as typed, server version, link security (only when weaker than fully checked), read-only, SAVE CONNECTION; right: one activity at a time",
        SPACE_8,
        |ui| {
            let red = shop(ConnectionColor::Red);
            let server = Some("PostgreSQL 14.24");
            let link = |words| Some((words, "The link's longer text, on hover."));
            let s = Duration::from_millis;
            let at = |address: &'static status::Address| Facts { address: Some(address), ..Facts::default() };
            stack(
                ui,
                vec![
                    (red, Facts { server, link: link("Not encrypted"), unsaved: true, ..at(&PG14) }, Some(Activity::InTransaction)),
                    (red, Facts { server, read_only: true, ..at(&PG14) }, Some(Activity::Disconnected)),
                    (
                        red,
                        Facts { link: link("Encrypted, server not checked"), ..at(&PG14) },
                        Some(Activity::Reconnecting { elapsed: Some(s(4100)) }),
                    ),
                    (
                        red,
                        Facts { link: link("Encrypted, certificate checked"), ..at(&PG14) },
                        Some(Activity::Exporting { elapsed: Some(s(12_400)) }),
                    ),
                    (red, at(&TUNNEL), Some(Activity::Importing { elapsed: Some(s(3900)) })),
                    (red, at(&SHOP_FILE), Some(Activity::Job { text: Words::from("Counting rows"), elapsed: Some(s(1200)), cancel: true })),
                ],
            );
        },
    );
    section(
        ui,
        "not on the page yet: a job before the app gives timers, today's spinner in the timer's place; a table's name keeps its case; a name dbee made, as written, with no address; a long path cut in the middle",
        SPACE_8,
        |ui| {
            let job = Activity::Job { text: Words::from("Loading ").name("Orders").plain("…"), elapsed: None, cancel: true };
            let made = Connection { name: "postgres@pg14.internal/Shop", colour: ConnectionColor::None, own: false };
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = STATUS_APART;
                stack(
                    ui,
                    vec![
                        (shop(ConnectionColor::Blue), Facts { address: pg14, ..Facts::default() }, Some(job)),
                        (made, Facts { server: Some("PostgreSQL 14.24"), ..Facts::default() }, None),
                    ],
                );
                ui.allocate_ui(vec2(LONG_PATH_W, SLIM_BAR_H), |ui| {
                    let facts = Facts { address: Some(&LONG_PATH), read_only: true, ..Facts::default() };
                    status::bar(ui, &shop(ConnectionColor::Red), &facts, None, |_| {});
                });
            });
        },
    );
}

/// The run row, the result tabs, the result bar and the row count, as the page's Tabs and bars
/// frame shows them, and their states before the app gives timers.
fn results(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    let stack = |ui: &mut Ui, rows: &mut dyn FnMut(&mut Ui)| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SPACE_12;
            rows(ui);
        });
    };
    let bar = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
        ui.allocate_ui(vec2(STRIP_W, BAR_H), |ui| add(ui));
    };
    section(
        ui,
        "run row: idle; running (RUN off, CANCEL the ink button); RUN CURRENT as an option, hidden by default: Kaspar hasn't decided on it",
        SPACE_8,
        |ui| {
            stack(ui, &mut |ui| {
                for (running, run_current) in [(false, false), (true, false), (false, true)] {
                    bar(ui, &mut |ui| {
                        RunRow { running, run_current, can_run: true }.show(ui);
                    });
                }
            });
        },
    );
    section(ui, "result tab: inactive, hover, active, focus (inferred)", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_16;
        ui.add(ResultTab::new("select ... series"));
        ui.add(ResultTab::new("select ... series").look(hovered));
        ui.add(ResultTab::new("select ... series").active(true));
        ui.add(ResultTab::new("select ... series").look(focused));
    });
    let two = || vec![ResultTab::new("select ... orders").active(true), ResultTab::new("select ... series")];
    let s = Duration::from_millis;
    section(
        ui,
        "result bar: done, running, failed, cancelled; new facts: 2 rows affected, first 10,000 of 25,000 rows; the value panel open (on)",
        SPACE_8,
        |ui| {
            let done = RunState::Done { rows: 10, ms: Some(24) };
            let rows = [
                ResultBar { tabs: two(), state: done, first_of: None, export: true, value_on: false },
                ResultBar { tabs: two(), state: RunState::Running { elapsed: Some(s(2400)) }, first_of: None, export: true, value_on: false },
                ResultBar { tabs: Vec::new(), state: RunState::Failed { ms: Some(12) }, first_of: None, export: false, value_on: false },
                ResultBar { tabs: Vec::new(), state: RunState::Cancelled { elapsed: Some(s(3200)) }, first_of: None, export: false, value_on: false },
                ResultBar { tabs: two(), state: RunState::Affected { rows: 2, ms: Some(5) }, first_of: None, export: false, value_on: false },
                ResultBar {
                    tabs: two(),
                    state: RunState::Done { rows: 10_000, ms: Some(80) },
                    first_of: Some((10_000, 25_000)),
                    export: true,
                    value_on: false,
                },
                ResultBar { tabs: two(), state: done, first_of: None, export: true, value_on: true },
            ];
            stack(ui, &mut |ui| {
                for row in &rows {
                    bar(ui, &mut |ui| {
                        row.clone().show(ui);
                    });
                }
            });
        },
    );
    section(
        ui,
        "rows/count: exact; estimate with COUNT; unknown with COUNT ROWS; counting with its timer (its CANCEL is the footer job)",
        SPACE_8,
        |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_32;
            let first = Some((1, 100));
            RowCount::Exact(25_000).show(ui, first);
            RowCount::Estimate(100_000).show(ui, first);
            RowCount::Unknown.show(ui, first);
            RowCount::Counting { estimate: Some(100_000), elapsed: Some(s(1200)) }.show(ui, first);
        },
    );
    section(
        ui,
        "not on the page yet: before the app gives timers, today's spinner in the timer's place; a result's name keeps its case",
        SPACE_8,
        |ui| {
            stack(ui, &mut |ui| {
                bar(ui, &mut |ui| {
                    let tabs = vec![ResultTab::new("select … Orders").active(true), ResultTab::new("select … Line_Items")];
                    ResultBar { tabs, state: RunState::Running { elapsed: None }, first_of: None, export: true, value_on: false }.show(ui);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SPACE_32;
                    RowCount::Counting { estimate: None, elapsed: None }.show(ui, Some((101, 200)));
                    RowCount::Exact(1).show(ui, Some((1, 1)));
                });
            });
        },
    );
}

/// The table bar, the filter bar and the paging bar, as the page's Tabs and bars frame shows
/// them, and a page loading before the app gives timers.
fn table_tab_bars(ui: &mut Ui) {
    let stack = |ui: &mut Ui, rows: &mut dyn FnMut(&mut Ui)| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SPACE_12;
            rows(ui);
        });
    };
    let bar = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
        ui.allocate_ui(vec2(STRIP_W, BAR_H), |ui| add(ui));
    };
    let s = Duration::from_millis;
    let first = Some((1, 100));
    let table = |view, loading, count| TableBar { view, loading, count, shown: first, columns: 8, can_refresh: true, can_add: true, value_on: false };
    section(
        ui,
        "table bar: data, structure, loading; the row count: estimate with COUNT, unknown with COUNT ROWS, counting (its CANCEL is the footer job)",
        SPACE_8,
        |ui| {
            let rows = [
                table(View::Data, None, RowCount::Exact(25_000)),
                table(View::Structure, None, RowCount::Estimate(25_000)),
                table(View::Data, Some(Loading { elapsed: Some(s(800)) }), RowCount::Exact(25_000)),
                table(View::Data, None, RowCount::Estimate(100_000)),
                table(View::Data, None, RowCount::Unknown),
                table(View::Data, None, RowCount::Counting { estimate: Some(100_000), elapsed: Some(s(1200)) }),
            ];
            stack(ui, &mut |ui| {
                for row in rows {
                    bar(ui, &mut |ui| {
                        row.show(ui);
                    });
                }
            });
        },
    );
    section(ui, "not on the page yet: a page loading before the app gives timers, today's spinner in the timer's place", SPACE_8, |ui| {
        bar(ui, &mut |ui| {
            table(View::Data, Some(Loading { elapsed: None }), RowCount::Exact(25_000)).show(ui);
        });
    });
    section(
        ui,
        "filter bar: filter row first (raw SQL toggle, column, operator, value, remove, add, APPLY), then AND rows; on card, 4 by 12 inset, line under",
        SPACE_8,
        |ui| {
            ui.vertical(|ui| {
                ui.set_width(STRIP_W);
                table_bars::filter_bar(ui, |ui| {
                    let rows = [("dealer_id", "equals (=)", "42"), ("status", "equals (=)", "completed"), ("total", "greater than (>)", "100")];
                    for (n, (column, op, value)) in rows.into_iter().enumerate() {
                        table_bars::filter_row(ui, |ui| {
                            if n == 0 {
                                ui.add(IconButton::boxed(Icon::Code));
                            } else {
                                table_bars::and_mark(ui);
                            }
                            Dropdown::new(("showcase-filter-column", n), column).width(FILTER_COLUMN_W).show(ui, |_| ());
                            Dropdown::new(("showcase-filter-op", n), op).width(FILTER_OP_W).show(ui, |_| ());
                            let mut value = value.to_owned();
                            ui.add(Input::data(&mut value).desired_width(FILTER_VALUE_W));
                            ui.add(IconButton::boxed(Icon::Minus));
                            if n == 0 {
                                ui.add(IconButton::boxed(Icon::Plus));
                                ui.add(BoxButton::outline("Apply"));
                            }
                        });
                    }
                });
            });
        },
    );
    section(ui, "paging bar: with staged changes (1 CHANGE, RESET, APPLY); without", SPACE_8, |ui| {
        stack(ui, &mut |ui| {
            for changes in [1, 0] {
                bar(ui, &mut |ui| {
                    PagingBar { page: "Page 1 of 250", back: false, forward: true, changes, can_apply: true }.show(ui);
                });
            }
        });
    });
}

/// The sidebar's filter and count, as the page's Tabs and bars frame shows them, and the
/// refresh's states the page doesn't draw.
fn sidebar_header(ui: &mut Ui) {
    let focused = Look { focused: true, ..Look::default() };
    let row = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui, usize)| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPACE_24;
            for n in 0..4 {
                ui.allocate_ui(vec2(SIDEBAR_W, BAR_H), |ui| add(ui, n));
            }
        });
    };
    section(
        ui,
        "sidebar header: the filter (placeholder, filled, focus) with REFRESH; the count under it (516 tables, 3 of 516 tables, 0 tables, loading)",
        SPACE_8,
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = SPACE_8;
                row(ui, &mut |ui, n| {
                    let (typed, look) = match n {
                        0 => ("", None),
                        1 => ("ord", None),
                        2 => ("ord", Some(focused)),
                        _ => return,
                    };
                    let mut text = typed.to_owned();
                    let filter = sidebar::Filter::new(&mut text, "Filter tables").refresh(sidebar::Refresh::Idle);
                    match look {
                        Some(look) => filter.look(look).show(ui),
                        None => filter.show(ui),
                    };
                });
                let counts = [Count::All(516, "tables"), Count::Some(3, 516, "tables"), Count::All(0, "tables"), Count::Loading];
                row(ui, &mut |ui, n| {
                    if let Some(count) = counts.get(n) {
                        count.show(ui);
                    }
                });
            });
        },
    );
    section(
        ui,
        "not on the page yet: the refresh while a refresh runs and off; the connection list's filter, no refresh; typed text keeps its case",
        SPACE_8,
        |ui| {
            row(ui, &mut |ui, n| {
                let (typed, refresh) = match n {
                    0 => ("", Some(sidebar::Refresh::Running)),
                    1 => ("", Some(sidebar::Refresh::Off)),
                    2 => ("", None),
                    _ => ("Order_Items", Some(sidebar::Refresh::Idle)),
                };
                let mut text = typed.to_owned();
                let placeholder = if refresh.is_some() { "Filter tables" } else { "Filter" };
                let filter = sidebar::Filter::new(&mut text, placeholder);
                match refresh {
                    Some(refresh) => filter.refresh(refresh).show(ui),
                    None => filter.show(ui),
                };
            });
        },
    );
}

/// The tabs and the strip, as the page's Tabs and bars frame shows them, and a busy tab.
fn tabs(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    section(
        ui,
        "tab: table and query; inactive, hover (shows close), active (unsaved dot, close), focus (inferred, like a bar button)",
        SPACE_8,
        |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_16;
            for (tab, title) in [(Tab::table as fn(&'static str) -> Tab<'static>, "orders [3]"), (Tab::query, "Query 1")] {
                tab(title).show(ui);
                tab(title).look(hovered).show(ui);
                tab(title).active(true).unsaved(true).show(ui);
                tab(title).look(focused).show(ui);
            }
        },
    );
    section(ui, "tab strip: tabs, then the new tab button (default, hover, focus), on card with the line under the inactive tabs", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_16;
        ui.allocate_ui(vec2(STRIP_W, TAB_H), |ui| {
            tab::strip(ui, |ui| {
                Tab::table("orders [3]").show(ui);
                Tab::table("customers [1]").show(ui);
                Tab::query("Query 1").active(true).unsaved(true).show(ui);
                ui.add(NewTab::new());
            });
        });
        ui.add(NewTab::new().look(hovered));
        ui.add(NewTab::new().look(focused));
    });
    section(ui, "not on the page yet: a busy tab, the spinner in its icon's place; inactive, active", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_16;
        Tab::table("orders").busy(true).show(ui);
        Tab::query("Query 1").busy(true).active(true).show(ui);
    });
}

/// The dialogs and the scrim, as the page's Dialogs frame shows them: two to a row, 24 apart and
/// 32 between rows.
fn dialogs(ui: &mut Ui) {
    let focused = Look { focused: true, ..Look::default() };
    let id = |n: &str| egui::Id::new("showcase-dialog").with(n);
    let cancel = Button::outline("Cancel", "dialog-cancel");
    section(
        ui,
        "dialog: Screens' dialogs from the components (database password, fill from URL with an error, close anyway, delete connection)",
        SPACE_8,
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(SPACE_24, SPACE_32);
                ui.horizontal_top(|ui| {
                    let (mut password, mut save, mut url) = ("secretpass".to_owned(), true, "shop@pg14:5432".to_owned());
                    Dialog::new(id("password"), "Database password")
                        .text(Words::from("Enter the password of ").name("shop").plain(" on ").name("pg14.internal").plain("."))
                        .body(|ui, _| {
                            ui.add(Input::body(&mut password).password(true).look(focused));
                            ui.add(Checkbox::new(&mut save, "Save in keyring"));
                        })
                        .buttons([Button::primary("Connect", "dialog-ok"), cancel])
                        .in_place(ui);
                    Dialog::new(id("url"), "Fill from URL")
                        .body(|ui, _| {
                            Field::unlabelled(Input::body(&mut url).error(true)).error(Some("Not a database URL")).show(ui);
                        })
                        .buttons([Button::primary("Fill", "dialog-ok"), cancel])
                        .in_place(ui);
                });
                ui.horizontal_top(|ui| {
                    Dialog::new(id("close"), "Close anyway?")
                        .text("Close Query 1? Its query text will be lost.")
                        .buttons([Button::danger("Close anyway", "dialog-ok"), cancel])
                        .in_place(ui);
                    Dialog::new(id("delete"), "Delete connection")
                        .text(Words::from("Delete “").name("Shop live").plain("”? Its saved passwords are deleted with it."))
                        .buttons([Button::danger("Delete", "dialog-ok"), cancel])
                        .in_place(ui);
                });
            });
        },
    );
    section(
        ui,
        "new: Apply again (an Apply whose outcome is unknown); the commit prompt on closing a tab or switching connections, COMMIT is the main button",
        SPACE_8,
        |ui| {
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = SPACE_24;
                Dialog::new(id("apply"), "Apply again?")
                    .text(
                        "Connection lost during Apply — the outcome is unknown. Reset and reload to check what was written, or Apply again to retry.",
                    )
                    .buttons([Button::danger("Apply again", "dialog-ok"), cancel])
                    .in_place(ui);
                // The page's placeholder words: the commit prompt isn't built yet.
                Dialog::new(id("commit"), "Commit the transaction?")
                    .text("Query 1 has changes that aren't committed.")
                    .buttons([Button::primary("Commit", "commit"), Button::outline("Rollback", "rollback"), cancel])
                    .in_place(ui);
            });
        },
    );
    section(ui, "scrim: over the whole window; the dialog centred, near the top", SPACE_8, |ui| {
        let t = tokens(ui.ctx());
        let (scene, _) = ui.allocate_exact_size(DIALOG_SCENE, Sense::hover());
        let p = ui.painter();
        p.rect_filled(scene, CornerRadius::ZERO, t.bg);
        Text::Code.paint(p, scene.min + BEHIND_AT, Align2::LEFT_TOP, BEHIND, t.text);
        p.rect_filled(scene, CornerRadius::ZERO, t.scrim);
        // The page draws the scene's edge over what it holds, the scrim too.
        p.rect_stroke(scene, CornerRadius::ZERO, Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        let at = UiBuilder::new().max_rect(Rect::from_min_size(scene.min + DIALOG_AT, DIALOG_SCENE)).layout(Layout::top_down(Align::Min));
        ui.scope_builder(at, |ui| {
            Dialog::new(id("scrim"), "Close anyway?")
                .text("Close Query 1? Its query text will be lost.")
                .buttons([Button::danger("Close anyway", "dialog-ok"), cancel])
                .in_place(ui);
        });
    });
}

/// The tooltips, the toasts and the toasts' icons, as the page's Tooltips and toasts frame shows
/// them.
fn tooltips_and_toasts(ui: &mut Ui) {
    section(ui, "tooltip: one line, wrapped at 320; 4 px below the pointer, its left edge at the pointer", SPACE_8, |ui| {
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_24;
            tooltip::in_place(ui, "New query tab (Ctrl+T)");
            tooltip::in_place(ui, "Next page: not now, changes are staged. Apply or reset them first, then go to the next page.");
        });
    });
    section(ui, "toast: success, error, error without an action; icon/success, icon/error", SPACE_8, |ui| {
        let t = tokens(ui.ctx());
        ui.spacing_mut().item_spacing.x = SPACE_24;
        ui.add(Toast::success(Words::from("Exported ").name("25,000").plain(" rows to ").name("orders.csv")).action("Open folder"));
        ui.add(Toast::error("Export failed: disk full").action("Details"));
        ui.add(Toast::error("Export failed: disk full"));
        for icon in [Icon::Success, Icon::Error] {
            let (cell, _) = ui.allocate_exact_size(icon.size(), Sense::hover());
            icon.paint(ui.painter(), cell.center(), t.text_muted);
        }
    });
}

/// The menus, their items and the menu bar's titles, as the page's Menus frame shows them.
fn menus(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    section(ui, "menu: Edit as on Screens; menu/item, menu/separator, keycaps", SPACE_8, |ui| {
        menu::in_place(ui, MENU_W, |ui| {
            ui.add(Item::new("Undo").shortcut("Ctrl+Z"));
            ui.add(Item::new("Redo").shortcut("Ctrl+Shift+Z"));
            menu::separator(ui);
            ui.add(Item::new("Copy").shortcut("Ctrl+C").look(hovered));
            ui.add(Item::new("Select All").shortcut("Ctrl+A"));
            menu::separator(ui);
            ui.add_enabled(false, Item::new("Apply").shortcut("Ctrl+S"));
            ui.add_enabled(false, Item::new("Reset"));
        });
    });
    section(ui, "menubar/title open, View menu, Theme submenu: the chosen mark sits in a left column", SPACE_8, |ui| {
        let (scene, _) = ui.allocate_exact_size(SCENE, Sense::hover());
        let titles = menu_bar(ui, scene.min, SCENE.x, &[("File", false), ("Edit", false), ("View", true), ("Query", false)]);
        let view = titles.get(2).map_or(scene.min.x, |r| r.left());
        let at = |pos| UiBuilder::new().max_rect(Rect::from_min_size(pos, SCENE)).layout(Layout::top_down(Align::Min));
        let shown = ui.scope_builder(at(pos2(view, scene.top() + SLIM_BAR_H)), |ui| {
            menu::in_place(ui, VIEW_MENU_W, |ui| {
                ui.add(Item::new("Refresh Schema"));
                ui.add(Item::new("Toggle Value Panel"));
                ui.add(Item::new("Theme").submenu(true).highlighted(true)).rect
            })
        });
        let (menu_rect, theme) = (shown.response.rect, shown.inner.inner);
        let edge = f32::from(MENU_PAD) + 1.0;
        ui.scope_builder(at(pos2(menu_rect.right() - SUBMENU_OVERLAP, theme.top() - edge)), |ui| {
            menu::in_place(ui, THEME_MENU_W, |ui| {
                for (words, chosen) in [("System", false), ("Light", true), ("Dark", false)] {
                    ui.add(Item::new(words).choice(Some(chosen)));
                }
            });
        });
    });
    section(
        ui,
        "menu/item: default, hover (the keyboard highlight looks the same), disabled, destructive, destructive hover; menubar/title: default, open",
        SPACE_8,
        |ui| {
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = SPACE_24;
                menu::in_place(ui, MENU_W, |ui| {
                    ui.add(Item::new("Copy").shortcut("Ctrl+C"));
                    ui.add(Item::new("Copy").shortcut("Ctrl+C").look(hovered));
                    ui.add_enabled(false, Item::new("Apply").shortcut("Ctrl+S"));
                    menu::separator(ui);
                    ui.add(Item::new("Delete row").destructive(true));
                    ui.add(Item::new("Delete row").destructive(true).look(hovered));
                });
                let (bar, _) = ui.allocate_exact_size(vec2(MENU_BAR_W, SLIM_BAR_H), Sense::hover());
                menu_bar(ui, bar.min, MENU_BAR_W, &[("File", false), ("Edit", true)]);
            });
        },
    );
}

/// A menu bar `width` wide at `at` holding `titles` (each with whether it is open), as the
/// page's Menus frame draws one ([`menu::bar`]). Returns where the titles are.
fn menu_bar(ui: &mut Ui, at: egui::Pos2, width: f32, titles: &[(&str, bool)]) -> Vec<Rect> {
    let mut bar = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(at, vec2(width, SLIM_BAR_H))).layout(Layout::top_down(Align::Min)));
    menu::bar(&mut bar, |ui| titles.iter().map(|(words, open)| ui.add(BarTitle::new(words).open(*open)).rect).collect())
}

/// The boxed and text buttons in each of their states, as the page's Buttons frame shows them.
fn buttons(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    section(ui, "button/box primary: default, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(BoxButton::primary("Run").shortcut("Ctrl+Enter"));
        ui.add(BoxButton::primary("Apply"));
        ui.add(BoxButton::primary("Save"));
        ui.add(BoxButton::primary("Apply").look(focused));
        ui.add_enabled(false, BoxButton::primary("Run").shortcut("Ctrl+Enter"));
    });
    section(ui, "button/box outline: default, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(BoxButton::outline("Cancel"));
        ui.add(BoxButton::outline("Cancel").look(hovered));
        ui.add(BoxButton::outline("Cancel").look(focused));
        ui.add_enabled(false, BoxButton::outline("Clear"));
    });
    section(ui, "button/box danger: default, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(BoxButton::danger("Delete"));
        ui.add(BoxButton::danger("Delete").look(focused));
        ui.add_enabled(false, BoxButton::danger("Delete"));
    });
    // Bar buttons stand side by side, as in a bar.
    section(ui, "button/bar: default, with divider, hover, on, disabled, focus", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add(BarButton::label(Icon::Refresh, "Refresh").divider(false));
        ui.add(BarButton::label(Icon::Plus, "Row"));
        ui.add(BarButton::label(Icon::Import, "Import").look(hovered));
        ui.add(BarButton::label(Icon::Wrap, "Wrap").on(true));
        ui.add_enabled(false, BarButton::label(Icon::Copy, "Copy"));
        ui.add(BarButton::label(Icon::Refresh, "Refresh").look(focused));
    });
    section(ui, "button/bar icon: default, hover, on, disabled, focus", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add(BarButton::icon(Icon::Panel).divider(false));
        ui.add(BarButton::icon(Icon::Panel).divider(false).look(hovered));
        ui.add(BarButton::icon(Icon::Panel).divider(false).on(true));
        ui.add_enabled(false, BarButton::icon(Icon::Panel).divider(false));
        ui.add(BarButton::icon(Icon::Refresh).look(focused));
    });
    section(ui, "button/text: default, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(TextButton::new("Reset"));
        ui.add(TextButton::new("Reset").look(hovered));
        ui.add(TextButton::new("Reset").look(focused));
        ui.add_enabled(false, TextButton::new("Reset"));
    });
    section(ui, "button/icon boxed: plus, minus, code, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(IconButton::boxed(Icon::Plus));
        ui.add(IconButton::boxed(Icon::Minus));
        ui.add(IconButton::boxed(Icon::Code));
        ui.add(IconButton::boxed(Icon::Plus).look(hovered));
        ui.add(IconButton::boxed(Icon::Plus).look(focused));
        ui.add_enabled(false, IconButton::boxed(Icon::Minus));
    });
    section(ui, "button/icon plain: first, previous, next, last, hover, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        for icon in [Icon::First, Icon::Previous, Icon::Next, Icon::Last] {
            ui.add(IconButton::plain(icon));
        }
        ui.add(IconButton::plain(Icon::Next).look(hovered));
        ui.add(IconButton::plain(Icon::Next).look(focused));
        ui.add_enabled(false, IconButton::plain(Icon::First));
    });
}

/// The inputs in each of their states and two fields, as the page's Field and input frame shows
/// them.
fn inputs(ui: &mut Ui) {
    let focused = Look { focused: true, ..Look::default() };
    let rows: [(&str, fn(&mut String) -> Input<'_>, &str, &str, &str); 2] = [
        ("input body: empty, placeholder, filled, focus, disabled, error", Input::body, "shop", "Optional", "shop"),
        ("input data: empty, placeholder, filled, focus, disabled, error", Input::data, "42", "None", "pg14.internal"),
    ];
    for (caption, new, typed, placeholder, wrong) in rows {
        section(ui, caption, SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = ROW_GAP;
            let mut text = [String::new(), String::new(), typed.to_owned(), typed.to_owned(), typed.to_owned(), wrong.to_owned()];
            let [empty, hint, filled, focus, off, error] = &mut text;
            ui.add(new(empty).desired_width(INPUT_W));
            ui.add(new(hint).placeholder(placeholder).desired_width(INPUT_W));
            ui.add(new(filled).desired_width(INPUT_W));
            ui.add(new(focus).look(focused).desired_width(INPUT_W));
            ui.add_enabled(false, new(off).desired_width(INPUT_W));
            ui.add(new(error).error(true).desired_width(INPUT_W));
        });
    }
    section(ui, "field: label plus input; with an error line", SPACE_8, |ui| {
        // The page's fields stand on one line, their tops level.
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = ROW_GAP;
            let (mut name, mut url) = ("shop".to_owned(), "shop".to_owned());
            ui.add(Field::new("Name", Input::body(&mut name).desired_width(INPUT_W)));
            ui.add(Field::new("Database URL", Input::body(&mut url).desired_width(INPUT_W)).error(Some("Not a database URL")));
        });
    });
}

/// The dropdown, checkbox, segmented control and swatch in each of their states, as the page's
/// Dropdown, Checkbox, Segmented and Swatch frames show them.
fn choices(ui: &mut Ui) {
    let hovered = Look { hovered: true, ..Look::default() };
    let focused = Look { focused: true, ..Look::default() };
    section(ui, "dropdown: default, focus, disabled", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        Dropdown::new("showcase-dropdown", "equals (=)").width(INPUT_W).show(ui, |_| ());
        Dropdown::new("showcase-dropdown-focus", "equals (=)").width(INPUT_W).look(focused).show(ui, |_| ());
        ui.add_enabled_ui(false, |ui| Dropdown::new("showcase-dropdown-off", "equals (=)").width(INPUT_W).show(ui, |_| ()));
    });
    section(ui, "checkbox: on, off, on disabled, off disabled, on focus, off focus", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = SPACE_24;
        for (mut on, enabled, look) in [
            (true, true, None),
            (false, true, None),
            (true, false, None),
            (false, false, None),
            (true, true, Some(focused)),
            (false, true, Some(focused)),
        ] {
            let c = Checkbox::new(&mut on, "Read-only");
            ui.add_enabled(enabled, if let Some(look) = look { c.look(look) } else { c });
        }
    });
    let views = [(0, "Data"), (1, "Structure")];
    let modes = [(0, "disable"), (1, "prefer"), (2, "require"), (3, "verify-ca"), (4, "verify-full")];
    let three = [(3, "verify-ca"), (4, "verify-full"), (1, "prefer")];
    section(ui, "segmented caps, 2 segments", SPACE_8, |ui| {
        ui.add(Segmented::caps(&mut 0, &views));
    });
    section(ui, "segmented small, 3 and 5 segments", SPACE_8, |ui| {
        ui.spacing_mut().item_spacing.x = ROW_GAP;
        ui.add(Segmented::small(&mut 1, &three).width(SEGMENTED_W));
        ui.add(Segmented::small(&mut 1, &modes).width(SEGMENTED_W));
    });
    let states = |ui: &mut Ui, segment: &dyn Fn(bool) -> Segment<'static>| {
        ui.spacing_mut().item_spacing.x = SPACE_12;
        ui.add(segment(true));
        ui.add(segment(false));
        ui.add(segment(false).look(hovered));
        ui.add_enabled(false, segment(false));
        ui.add_enabled(false, segment(true));
        ui.add(segment(true).look(focused));
        ui.add(segment(false).look(focused));
    };
    section(ui, "segmented/segment: selected, default, hover, disabled, selected disabled, selected focus, focus", SPACE_8, |ui| {
        states(ui, &|selected| Segment::caps("Data", selected));
    });
    section(ui, "segmented/segment small: selected, default, hover, disabled, selected disabled, selected focus, focus", SPACE_8, |ui| {
        states(ui, &|selected| Segment::small(if selected { "prefer" } else { "require" }, selected).width(SMALL_SEGMENT_W));
    });
    for (caption, selected) in [("swatch: not selected; the last one has focus", false), ("swatch: selected; the last one has focus", true)] {
        section(ui, caption, SPACE_8, |ui| {
            ui.spacing_mut().item_spacing.x = SPACE_12;
            for colour in COLOURS {
                ui.add(Swatch::new(colour, selected));
            }
            ui.add(Swatch::new(ConnectionColor::Red, selected).look(focused));
        });
    }
}

/// A section: the page's caption, `gap` under it, then `row`.
fn section(ui: &mut Ui, caption: &str, gap: f32, row: impl FnOnce(&mut Ui)) {
    let t = tokens(ui.ctx());
    ui.add(Label::new(Text::Label.rich(caption).color(t.text_muted)).selectable(false));
    ui.add_space(gap);
    ui.horizontal(row);
    ui.add_space(BETWEEN);
}

/// Each icon with its name, in its usual colour.
fn icons(ui: &mut Ui) {
    let t = tokens(ui.ctx());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = ICON_CELL_GAP;
        for row in ICONS.chunks(ICONS_PER_ROW) {
            ui.horizontal(|ui| {
                for &icon in row {
                    let (cell, _) = ui.allocate_exact_size(ICON_CELL, Sense::hover());
                    let p = ui.painter();
                    let bottom = cell.top() + ICON_BOTTOM;
                    if icon == Icon::CheckBox {
                        let tile = Rect::from_center_size(pos2(cell.center().x, bottom - ICON_TILE / 2.0), Vec2::splat(ICON_TILE));
                        p.rect_filled(tile, CornerRadius::same(RADIUS_CONTROL), t.ink);
                        icon.paint(p, tile.center(), t.on_ink);
                    } else {
                        icon.paint(p, pos2(cell.center().x, bottom - icon.size().y / 2.0), usual(icon, t));
                    }
                    Text::Small.paint(p, pos2(cell.center().x, cell.top() + ICON_NAME_TOP), Align2::CENTER_TOP, icon.name(), t.text_muted);
                }
            });
        }
    });
}

/// The colour the page's Icons frame draws `icon` in.
fn usual(icon: Icon, t: &Tokens) -> Color32 {
    match icon {
        Icon::Key | Icon::KeyPrimary => t.key,
        Icon::SortAscending | Icon::SortDescending => t.ink,
        Icon::CheckBox => t.on_ink,
        Icon::ChevronDown
        | Icon::ChevronRight
        | Icon::Close
        | Icon::TabTable
        | Icon::Check
        | Icon::Search
        | Icon::NoColour
        | Icon::Schema
        | Icon::Table
        | Icon::View
        | Icon::MaterializedView
        | Icon::KeyForeign => t.text_muted,
        _ => t.text,
    }
}
