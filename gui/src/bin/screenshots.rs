//! Renders the real app headlessly against a seeded demo database and
//! writes the PNG screenshots used by the README and the docs.
//!
//! ```sh
//! mise run gui:screenshots            # all scenarios → docs/public/screenshots
//! cargo run -p dbcrust-gui --features screenshots --bin dbcrust-gui-screenshots -- \
//!     --out /tmp/shots --only query     # one scenario, anywhere
//! ```
//!
//! Config is isolated in a temp `DBCRUST_CONFIG_DIR`, so your own sessions
//! and recent connections never appear (or change).
//!
//! `--postgres <url>` also seeds the demo schema into that PostgreSQL
//! database (it drops and recreates the demo tables!) and uses it for the
//! query, plan (EXPLAIN ANALYZE) and inspector shots. `mise run
//! gui:screenshots` starts a throwaway container for it.

use dbcrust_gui::actions;
use dbcrust_gui::app_state::AppState;
use dbcrust_gui_core::gui_state::ThemeModeSetting;
use dbcrust_gui_core::{ConfigStore, ConnectRequest, ConnectionHandle, RequestId};
use gpui_kit::component::ThemeMode;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;
use image::{Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SHOWCASE_QUERY: &str = "-- Top customers by lifetime value\nSELECT c.name AS customer,\n       c.country,\n       COUNT(DISTINCT o.id) AS orders,\n       ROUND(SUM(oi.quantity * oi.unit_price), 2) AS lifetime_value,\n       MAX(o.ordered_at) AS last_order\nFROM customers c\nJOIN orders o ON o.customer_id = c.id\nJOIN order_items oi ON oi.order_id = o.id\nWHERE o.status <> 'cancelled'\nGROUP BY c.id\nORDER BY lifetime_value DESC\nLIMIT 50;\n";

struct Args {
    out: PathBuf,
    only: Option<String>,
    frame: bool,
    demo_dir: Option<PathBuf>,
    postgres: Option<String>,
    /// `--size 960x600`: window size in logical pixels (default: the app's).
    size: Option<Size<Pixels>>,
    /// `--theme "Tokyo Night"`: a bundled theme for the dark scenarios.
    theme: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        out: PathBuf::from("docs/public/screenshots"),
        only: None,
        frame: true,
        demo_dir: None,
        postgres: None,
        size: None,
        theme: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--out" => args.out = PathBuf::from(iter.next().expect("--out needs a path")),
            "--only" => args.only = iter.next(),
            "--no-frame" => args.frame = false,
            "--theme" => args.theme = Some(iter.next().expect("--theme needs a name")),
            "--postgres" => args.postgres = Some(iter.next().expect("--postgres needs a URL")),
            "--size" => {
                let value = iter.next().expect("--size needs WIDTHxHEIGHT");
                let (width, height) = value.split_once('x').expect("--size needs WIDTHxHEIGHT");
                args.size = Some(size(
                    px(width.parse().expect("a width")),
                    px(height.parse().expect("a height")),
                ));
            }
            "--demo-dir" => {
                args.demo_dir = Some(PathBuf::from(iter.next().expect("--demo-dir needs a path")))
            }
            other => panic!("unknown argument {other}"),
        }
    }
    args
}

fn main() {
    let args = parse_args();
    let temp = tempfile::tempdir().expect("temp dir");
    // SAFETY: single-threaded at this point
    unsafe { std::env::set_var("DBCRUST_CONFIG_DIR", temp.path().join("config")) };
    std::fs::create_dir_all(&args.out).expect("output directory");

    // Visible in the status bar: `mise run gui:screenshots` passes a short
    // `--demo-dir`; the per-process default keeps concurrent runs apart
    let demo_dir = args.demo_dir.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("dbcrust-demo-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&demo_dir);
    std::fs::create_dir_all(&demo_dir).expect("demo directory");
    let db_path = demo_dir.join("shop.db");
    seed_demo_database(&db_path);
    let demo_url = format!("sqlite://{}", db_path.display());
    if let Some(postgres) = &args.postgres {
        seed_postgres(postgres);
    }

    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(dbcrust_gui::assets()),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.allow_parking();
    if args.only.as_deref() == Some("home-first-run") {
        // A brand-new install: nothing saved, nothing recent, no containers
        // (the demo database seeded above recorded a recent entry)
        let config = temp.path().join("config");
        std::fs::create_dir_all(&config).expect("config dir");
        let _ = std::fs::remove_file(config.join("recent.toml"));
        dbcrust_gui_core::docker::set_discovery_override(
            dbcrust_gui_core::docker::DockerDiscovery::Containers(Vec::new()),
        );
    } else {
        seed_home_fixtures(&temp.path().join("config"));
    }
    let store = ConfigStore::load();
    if args
        .only
        .as_deref()
        .is_some_and(|only| only.starts_with("settings-"))
    {
        use dbcrust_gui_core::DatabaseType;
        use dbcrust_gui_core::dbcrust::config::NamedQueryScope;
        let postgres = || NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL);
        let sqlite = || NamedQueryScope::DatabaseType(DatabaseType::SQLite);
        for (name, sql, scope, description) in [
            (
                "active_customers",
                "-- Customers who ordered recently\nSELECT id, name, country, last_order_at\nFROM customers\nWHERE last_order_at > CURRENT_DATE - $1::interval\nORDER BY last_order_at DESC\nLIMIT $2;",
                postgres(),
                "Customers with an order in the last N days.",
            ),
            (
                "orders_by_country",
                "SELECT country, COUNT(*) AS orders\nFROM orders\nGROUP BY country\nORDER BY orders DESC;",
                NamedQueryScope::Global,
                "Order volume per country.",
            ),
            (
                "recent_orders",
                "SELECT * FROM orders WHERE customer_id = $1 ORDER BY created_at DESC LIMIT 20;",
                sqlite(),
                "The last orders of one customer.",
            ),
            (
                "slow_statements",
                "SELECT query, calls, mean_exec_time\nFROM pg_stat_statements\nORDER BY mean_exec_time DESC\nLIMIT 20;",
                postgres(),
                "",
            ),
            (
                "table_sizes",
                "SELECT relname, pg_size_pretty(pg_total_relation_size(relid))\nFROM pg_catalog.pg_statio_user_tables\nORDER BY pg_total_relation_size(relid) DESC;",
                postgres(),
                "Largest tables first, indexes included.",
            ),
            (
                "daily_signups",
                "SELECT date_trunc('day', created_at) AS day, COUNT(*)\nFROM users\nWHERE created_at >= $1\nGROUP BY 1 ORDER BY 1;",
                NamedQueryScope::Global,
                "",
            ),
            (
                "health_check",
                "SELECT 1;",
                NamedQueryScope::Session("warehouse-prod".into()),
                "Connectivity check for this saved session.",
            ),
            (
                "top_products",
                "SELECT product_id, SUM(quantity) AS sold\nFROM order_items GROUP BY product_id\nORDER BY sold DESC LIMIT $1;",
                sqlite(),
                "",
            ),
        ] {
            store
                .update_named_query(None, name, sql, scope, Some(description.into()))
                .expect("seed saved query");
        }
        store.update_settings(|config| {
            for (pattern, target) in [
                (r"^db\.internal\..*$", "deploy@bastion.example.com:2222"),
                (
                    r"^warehouse\.prod$",
                    "analyst:s3cret@warehouse-gw.example.com",
                ),
                (r"\.rds\.amazonaws\.com$", "ops@`bastion-for rds`"),
            ] {
                config
                    .ssh_tunnel_patterns
                    .insert(pattern.into(), target.into());
            }
        });
        store.save_settings().expect("seed tunnels");
    }
    cx.update(|cx| dbcrust_gui::init(store, cx));

    let (window, workspace) = cx
        .update(|cx| {
            let mut options = dbcrust_gui::main_window_options(cx);
            options.window_bounds = Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: args.size.unwrap_or(dbcrust_gui::WINDOW_SIZE),
            }));
            options.focus = false;
            options.show = false;
            dbcrust_gui::open_main_window(options, cx)
        })
        .expect("open window");

    let mut shots = Shots {
        cx: &mut cx,
        window,
        args: &args,
    };

    set_theme(shots.cx, ThemeMode::Dark);
    if let Some(name) = args.theme.clone() {
        shots.cx.update(|cx| {
            dbcrust_gui::theme::set_theme(Some(name.into()), cx);
        });
        shots.cx.run_until_parked();
    }
    shots.capture("home");

    if let Some(name) = args
        .only
        .as_deref()
        .filter(|name| name.starts_with("settings-"))
    {
        use dbcrust_gui::settings::SettingsPage;
        let page = if name == "settings-ssh-tunnels" {
            SettingsPage::SshTunnels
        } else {
            SettingsPage::SavedQueries
        };
        shots.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| state.open_settings_page(page, cx))
        });
        shots.settle(Duration::from_millis(600));
        if page == SettingsPage::SshTunnels {
            let workspace = workspace.clone();
            shots.with_window(|window, cx| {
                if let Some(settings) = workspace.read(cx).settings_view() {
                    settings.update(cx, |settings, cx| {
                        settings.test_tunnel_host("db.internal.example.com", window, cx)
                    });
                }
            });
            shots.settle(Duration::from_millis(200));
        }
        shots.capture(name);
        println!("screenshots written to {}", args.out.display());
        let _ = std::fs::remove_dir_all(&demo_dir);
        return;
    }

    // Extra states of the Connections page, rendered on request only
    // (`--only home-new`, `home-docker`, …); they are not docs screenshots
    if args
        .only
        .as_deref()
        .is_some_and(|only| only.starts_with("home-"))
    {
        home_variants(&mut shots, &workspace);
        println!("screenshots written to {}", args.out.display());
        let _ = std::fs::remove_dir_all(&demo_dir);
        return;
    }

    // Connect to the demo database(s) and wait for their schemas. The last
    // one connected (PostgreSQL when given) becomes the active connection.
    let mut urls = vec![demo_url.clone()];
    urls.extend(args.postgres.clone());
    for url in &urls {
        let url = url.clone();
        shots.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.connect(ConnectRequest::url(url), cx).detach();
            })
        });
        shots.wait_until(Duration::from_secs(30), |cx| {
            let state = AppState::global(cx).read(cx);
            !state.is_connecting()
                && state
                    .connections()
                    .iter()
                    .all(|live| live.schema.tables().is_some())
        });
    }
    assert_eq!(
        shots
            .cx
            .update(|cx| AppState::global(cx).read(cx).connections().len()),
        urls.len(),
        "every demo connection opened"
    );

    let connection = shots
        .cx
        .update(|cx| AppState::global(cx).read(cx).active_id())
        .expect("demo connection");
    shots.cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.open_query(
                connection,
                Some("Top customers".into()),
                SHOWCASE_QUERY.into(),
                true,
                cx,
            );
        })
    });
    shots.settle(Duration::from_millis(1500));
    shots.capture("query");
    query_states(&mut shots);

    // Explorer: one table selected and open, another one hovered
    shots.dispatch(Box::new(actions::FocusExplorer));
    for key in ["down", "down", "down", "down", "down", "down", "right"] {
        shots
            .cx
            .update_window(shots.window, |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            })
            .ok();
    }
    shots.settle(Duration::from_millis(600));
    shots
        .cx
        .update_window(shots.window, |_, window, cx| {
            // the "customers" row: below the title bar, header and filter
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(110.), px(186.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })
        .ok();
    shots.settle(Duration::from_millis(300));
    shots.capture("explorer");

    if args.postgres.is_some() {
        shots.dispatch(Box::new(actions::ExplainAnalyze));
    } else {
        shots.dispatch(Box::new(actions::ExplainStatement));
    }
    shots.settle(Duration::from_millis(1500));
    shots.capture("explain");

    shots.cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            let orders = state
                .active()
                .and_then(|live| live.schema.tables().cloned())
                .and_then(|tables| tables.iter().find(|t| t.name == "orders").cloned());
            if let Some(orders) = orders {
                state.open_table(connection, orders, cx);
            }
        })
    });
    shots.settle(Duration::from_millis(1500));
    shots.capture("inspector");

    shots.dispatch(Box::new(actions::CommandPalette));
    shots.settle(Duration::from_millis(500));
    shots.capture("palette");
    shots
        .cx
        .update_window(shots.window, |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        })
        .ok();

    shots
        .cx
        .update(|cx| AppState::global(cx).update(cx, |state, cx| state.open_settings(cx)));
    shots.settle(Duration::from_millis(500));
    shots.capture("settings");

    // Light theme variant of the main screen
    shots.cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.open_query(
                connection,
                Some("Top customers".into()),
                SHOWCASE_QUERY.into(),
                true,
                cx,
            );
        })
    });
    set_theme(shots.cx, ThemeMode::Light);
    shots.settle(Duration::from_millis(1500));
    shots.capture("query-light");

    println!("screenshots written to {}", args.out.display());
    let _ = std::fs::remove_dir_all(&demo_dir);
}

/// Review-only states of the query workspace (`--only query-sorted`,
/// `query-stale`, `query-narrow`): never part of the docs set, and each one
/// ends the run so later scenarios do not see the changed state.
fn query_states(shots: &mut Shots) {
    let Some(name) = shots.args.only.clone() else {
        return;
    };
    match name.as_str() {
        // A local sort: arrow on the sorted column, the note in the footer,
        // and the arrow of a hovered header
        "query-sorted" => {
            shots.with_window(|window, cx| {
                window.render_frame(cx);
                window.click(("result-th", 3usize), cx);
                window.render_frame(cx);
                window.hover(("result-th", 1usize), cx);
            });
        }
        // The editor changed after the run
        "query-stale" => {
            shots.with_window(|window, cx| {
                // End of the first line, inside the statement that ran
                window.press("end", cx);
                window.input(" (v2)", cx);
            });
        }
        // Queries separated by blank lines, no semicolons: the one under the
        // caret (the second) is tinted
        "query-statements" | "query-statements-light" => {
            let light = name == "query-statements-light";
            if light {
                set_theme(shots.cx, ThemeMode::Light);
            }
            shots.with_window(|window, cx| {
                window.press("cmd-a", cx);
                window.input(
                    "-- Open orders\nselect * from orders where status = 'open'\n\nselect count(*) from customers\n\nselect c.name, sum(o.total) as spent\nfrom customers c join orders o on o.customer_id = c.id\ngroup by c.id",
                    cx,
                );
                // Dark: the caret in the second query; light: in the third,
                // which spans three lines
                for _ in 0..(if light { 1 } else { 4 }) {
                    window.press("up", cx);
                }
                window.press("end", cx);
            });
        }
        // Run with `--size 960x600`: the toolbar folds its secondary menus away
        "query-narrow" => {}
        // Many tabs: two connections that share a name, a table tab, a
        // long title, a renamed tab and one whose connection is closed.
        // `query-tabs-rename` leaves the title editor open.
        "query-tabs" | "query-tabs-rename" | "query-tabs-light" => {
            if name == "query-tabs-light" {
                set_theme(shots.cx, ThemeMode::Light);
            }
            tab_strip_states(shots, name == "query-tabs-rename")
        }
        _ => return,
    }
    shots.settle(Duration::from_millis(600));
    shots.capture_named(&name);
    std::process::exit(0);
}

/// A second and third database called `shop.db` in other folders, so the tab
/// strip has to tell connections of the same name apart.
fn tab_strip_states(shots: &mut Shots, editing: bool) {
    let root = std::env::temp_dir().join("dbcrust-demo-tabs-extra");
    let _ = std::fs::remove_dir_all(&root);
    let mut urls = Vec::new();
    for folder in ["b", "c"] {
        let directory = root.join(folder);
        std::fs::create_dir_all(&directory).expect("extra demo directory");
        let path = directory.join("shop.db");
        seed_demo_database(&path);
        urls.push(format!("sqlite://{}", path.display()));
    }
    for (n, url) in urls.into_iter().enumerate() {
        shots.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.connect(ConnectRequest::url(url), cx).detach();
            })
        });
        shots.wait_until(Duration::from_secs(30), move |cx| {
            let state = AppState::global(cx).read(cx);
            state.connections().len() == n + 2
                && state
                    .connections()
                    .iter()
                    .all(|live| live.schema.tables().is_some())
        });
    }
    let ids: Vec<_> = shots.cx.update(|cx| {
        AppState::global(cx)
            .read(cx)
            .connections()
            .iter()
            .map(|live| live.id())
            .collect()
    });
    let (second, third) = (ids[1], ids[2]);
    shots.cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.open_query(third, Some("Scratch".into()), "select 1".into(), false, cx);
            state.open_query(
                second,
                Some("Quarterly revenue by region and product category".into()),
                "select 1".into(),
                false,
                cx,
            );
            let orders = state
                .connection(second)
                .and_then(|live| live.schema.tables().cloned())
                .and_then(|tables| tables.iter().find(|t| t.name == "orders").cloned());
            if let Some(orders) = orders {
                state.open_table(second, orders, cx);
            }
            // Its connection goes away: the draft stays, unbound
            state.disconnect(third, cx);
            state.open_query(second, Some("Query 2".into()), "select 2".into(), false, cx);
        })
    });
    shots.settle(Duration::from_millis(800));
    shots.dispatch(Box::new(actions::RenameTab));
    shots.settle(Duration::from_millis(300));
    if !editing {
        shots.with_window(|window, cx| {
            window.input("Weekly report", cx);
            window.press("enter", cx);
        });
    }
}

struct Shots<'a> {
    cx: &'a mut HeadlessAppContext,
    window: AnyWindowHandle,
    args: &'a Args,
}

impl Shots<'_> {
    /// Pump GPUI tasks while worker threads make progress.
    fn settle(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
        self.cx.run_until_parked();
    }

    fn wait_until(&mut self, timeout: Duration, done: impl Fn(&mut App) -> bool) {
        let deadline = Instant::now() + timeout;
        loop {
            self.cx.run_until_parked();
            if self.cx.update(|cx| done(cx)) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for the app");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn dispatch(&mut self, action: Box<dyn Action>) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_action(action, cx)
            })
            .ok();
    }

    fn with_window(&mut self, change: impl FnOnce(&mut Window, &mut App)) {
        self.cx
            .update_window(self.window, |_, window, cx| change(window, cx))
            .expect("window");
    }

    fn capture(&mut self, name: &str) {
        if self.args.only.as_deref().is_some_and(|only| only != name) {
            return;
        }
        self.capture_named(name);
    }

    fn capture_named(&mut self, name: &str) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.render_frame(cx))
            .expect("render");
        let image = self
            .cx
            .capture_screenshot(self.window)
            .expect("offscreen renderer");
        let image = if self.args.frame {
            macos_frame(image)
        } else {
            image
        };
        let path = self.args.out.join(format!("gui-{name}.png"));
        save_png(&image, &path);
        println!("  {}", path.display());
    }
}

fn set_theme(cx: &mut HeadlessAppContext, mode: ThemeMode) {
    // Through the app's own theme module, so the surface hierarchy applies
    let setting = if mode.is_dark() {
        ThemeModeSetting::Dark
    } else {
        ThemeModeSetting::Light
    };
    cx.update(|cx| dbcrust_gui::theme::set_mode(setting, cx));
    cx.run_until_parked();
}

fn save_png(image: &RgbaImage, path: &Path) {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let file = std::fs::File::create(path).expect("create png");
    PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        CompressionType::Best,
        FilterType::Adaptive,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )
    .expect("encode png");
}

/// Rounded corners, macOS traffic lights and a soft shadow, so README
/// screenshots look like a real window (headless rendering has no OS
/// chrome).
fn macos_frame(window: RgbaImage) -> RgbaImage {
    let scale = 2.0_f32; // headless renders at 2x
    let radius = 10.0 * scale;
    let margin = (36.0 * scale) as u32;
    let (w, h) = window.dimensions();
    let mut out = RgbaImage::from_pixel(w + margin * 2, h + margin * 2, Rgba([0, 0, 0, 0]));

    // Shadow: blurred rounded rect, offset down
    let shadow_offset = 12.0 * scale;
    let blur = 28.0 * scale;
    for y in 0..out.height() {
        for x in 0..out.width() {
            let px_x = x as f32 - margin as f32;
            let px_y = y as f32 - margin as f32 - shadow_offset;
            let d = rounded_rect_distance(px_x, px_y, w as f32, h as f32, radius);
            let alpha = (1.0 - (d / blur).clamp(0.0, 1.0)).powf(2.2) * 0.38;
            if alpha > 0.0 {
                out.put_pixel(x, y, Rgba([0, 0, 0, (alpha * 255.0) as u8]));
            }
        }
    }

    // Window with anti-aliased rounded corners
    for y in 0..h {
        for x in 0..w {
            let d =
                rounded_rect_distance(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius);
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let src = window.get_pixel(x, y);
            let dst = out.get_pixel_mut(x + margin, y + margin);
            *dst = blend(
                *dst,
                Rgba([src[0], src[1], src[2], (coverage * 255.0) as u8]),
            );
        }
    }

    // Traffic lights (GPUI Kit's TitleBar leaves room for them on macOS)
    let lights = [[255, 95, 87], [254, 188, 46], [40, 200, 64]];
    let cy = margin as f32 + 18.0 * scale;
    for (i, color) in lights.iter().enumerate() {
        let cx = margin as f32 + (20.0 + i as f32 * 20.0) * scale;
        draw_circle(&mut out, cx, cy, 6.0 * scale, *color);
    }

    // 1px hairline border
    for y in 0..h {
        for x in 0..w {
            let d =
                rounded_rect_distance(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius);
            if (-scale..=0.0).contains(&d) {
                let dst = out.get_pixel_mut(x + margin, y + margin);
                *dst = blend(*dst, Rgba([128, 128, 128, 70]));
            }
        }
    }
    out
}

fn rounded_rect_distance(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn draw_circle(image: &mut RgbaImage, cx: f32, cy: f32, r: f32, color: [u8; 3]) {
    let (x0, x1) = ((cx - r - 1.0) as u32, (cx + r + 1.0) as u32);
    let (y0, y1) = ((cy - r - 1.0) as u32, (cy + r + 1.0) as u32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() - r;
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            if coverage > 0.0 {
                let dst = image.get_pixel_mut(x, y);
                *dst = blend(
                    *dst,
                    Rgba([color[0], color[1], color[2], (coverage * 255.0) as u8]),
                );
            }
        }
    }
}

fn blend(dst: Rgba<u8>, src: Rgba<u8>) -> Rgba<u8> {
    let sa = src[3] as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let channel =
        |i: usize| ((src[i] as f32 * sa + dst[i] as f32 * da * (1.0 - sa)) / out_a).round() as u8;
    Rgba([
        channel(0),
        channel(1),
        channel(2),
        (out_a * 255.0).round() as u8,
    ])
}

/// Saved sessions, recent connections and Docker containers for the home
/// screen — fictional, so nobody's real infrastructure shows up in docs.
fn seed_home_fixtures(config_dir: &Path) {
    use dbcrust_gui_core::DatabaseType;
    use dbcrust_gui_core::docker::{DockerContainer, DockerDiscovery, set_discovery_override};
    std::fs::create_dir_all(config_dir).expect("config dir");
    std::fs::write(
        config_dir.join("sessions.toml"),
        r#"[sessions.prod-analytics]
host = "warehouse.internal"
port = 5432
user = "analyst"
dbname = "analytics"
database_type = "PostgreSQL"

[sessions.staging-api]
host = "staging-db.internal"
port = 3306
user = "api"
dbname = "orders"
database_type = "MySQL"

[sessions.clickhouse-events]
host = "events.internal"
port = 8123
user = "default"
dbname = "events"
database_type = "ClickHouse"

[sessions.search-logs]
host = "search.internal"
port = 9200
user = "elastic"
dbname = ""
database_type = "Elasticsearch"
"#,
    )
    .expect("sessions.toml");

    let ago = |minutes: i64| (chrono::Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
    let recents = [
        (
            "postgres://analyst@warehouse.internal:5432/analytics",
            "analyst@warehouse.internal:5432/analytics",
            "PostgreSQL",
            12,
        ),
        ("sqlite:///Users/demo/data/shop.db", "shop.db", "SQLite", 95),
        (
            "parquet:///Users/demo/exports/events-2025-*.parquet",
            "events-2025-*.parquet",
            "Parquet",
            60 * 26,
        ),
        (
            "mysql://api@staging-db.internal:3306/orders",
            "api@staging-db.internal:3306/orders",
            "MySQL",
            60 * 24 * 3,
        ),
    ];
    let recent_toml: String = recents
        .iter()
        .map(|(url, display, db_type, minutes)| {
            format!(
                "[[connections]]\nconnection_url = \"{url}\"\ndisplay_name = \"{display}\"\ntimestamp = \"{}\"\ndatabase_type = \"{db_type}\"\nsuccess = true\n\n",
                ago(*minutes)
            )
        })
        .collect();
    std::fs::write(config_dir.join("recent.toml"), recent_toml).expect("recent.toml");

    let container =
        |name: &str, image: &str, db_type, port: u16, running: bool, published: bool| {
            DockerContainer {
                id: format!("{name}-id"),
                name: name.to_string(),
                image: image.to_string(),
                status: if running {
                    "Up 3 hours".into()
                } else {
                    "Exited (0) 2 days ago".into()
                },
                db_type: Some(db_type),
                host_port: (running && published).then_some(port),
                container_port: Some(port),
                running,
            }
        };
    // One published, one reached automatically, one stopped
    set_discovery_override(DockerDiscovery::Containers(vec![
        container(
            "shop-postgres",
            "postgres:17",
            DatabaseType::PostgreSQL,
            5432,
            true,
            true,
        ),
        container(
            "billing-postgres-1",
            "postgres:16.1",
            DatabaseType::PostgreSQL,
            5432,
            true,
            false,
        ),
        container(
            "legacy-mongo",
            "mongo:7",
            DatabaseType::MongoDB,
            27017,
            false,
            false,
        ),
    ]));
}

/// Recreate the demo schema in PostgreSQL and gather statistics so the plan
/// shows realistic estimates.
fn seed_postgres(url: &str) {
    SimpleRuntime.block_on(async {
        let store = ConfigStore::load();
        let (db, _) = ConnectionHandle::connect(ConnectRequest::url(url), store)
            .await
            .unwrap_or_else(|e| panic!("open {url}: {e}"));
        let mut statements = vec![
            "DROP VIEW IF EXISTS revenue_by_month".to_string(),
            "DROP TABLE IF EXISTS order_items, orders, customers, products, categories CASCADE"
                .to_string(),
        ];
        statements.extend(demo_sql());
        statements.push("ANALYZE".to_string());
        for statement in statements {
            db.execute(RequestId::next(), statement)
                .await
                .unwrap_or_else(|e| panic!("postgres seed failed: {e}"));
        }
        db.close();
    });
}

/// A small, realistic shop database.
fn seed_demo_database(path: &Path) {
    std::fs::File::create(path).expect("demo db file");
    let url = format!("sqlite://{}", path.display());
    SimpleRuntime.block_on(async {
        let store = ConfigStore::load();
        let (db, _) = ConnectionHandle::connect(ConnectRequest::url(url), store)
            .await
            .expect("open demo db");
        for statement in demo_sql() {
            db.execute(RequestId::next(), statement)
                .await
                .unwrap_or_else(|e| panic!("seed failed: {e}"));
        }
        db.close();
    });
}

/// Minimal executor for the seeding step: the worker owns its own tokio
/// runtime, so awaiting its oneshot replies only needs a polling loop.
struct SimpleRuntime;

impl SimpleRuntime {
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct ThreadWaker(std::thread::Thread);
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::park_timeout(Duration::from_millis(50));
        }
    }
}

fn demo_sql() -> Vec<String> {
    let mut sql: Vec<String> = [
        "CREATE TABLE categories (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE)",
        "CREATE TABLE products (id INTEGER PRIMARY KEY, sku TEXT NOT NULL UNIQUE, name TEXT NOT NULL, category_id INTEGER NOT NULL REFERENCES categories(id), price NUMERIC NOT NULL, stock INTEGER NOT NULL DEFAULT 0, discontinued_at TEXT)",
        "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL UNIQUE, country TEXT NOT NULL, created_at TEXT NOT NULL, marketing_opt_in INTEGER NOT NULL DEFAULT 0)",
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id), status TEXT NOT NULL CHECK (status IN ('pending','paid','shipped','delivered','cancelled')), ordered_at TEXT NOT NULL, shipped_at TEXT, notes TEXT)",
        "CREATE TABLE order_items (order_id INTEGER NOT NULL REFERENCES orders(id), product_id INTEGER NOT NULL REFERENCES products(id), quantity INTEGER NOT NULL, unit_price NUMERIC NOT NULL, PRIMARY KEY (order_id, product_id))",
        "CREATE INDEX orders_customer_idx ON orders(customer_id)",
        "CREATE INDEX orders_status_ordered_idx ON orders(status, ordered_at)",
        "CREATE INDEX order_items_product_idx ON order_items(product_id)",
        "CREATE VIEW revenue_by_month AS SELECT substr(o.ordered_at, 1, 7) AS month, ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue FROM orders o JOIN order_items oi ON oi.order_id = o.id WHERE o.status <> 'cancelled' GROUP BY 1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let categories = ["Coffee", "Tea", "Brewing gear", "Accessories", "Gift cards"];
    sql.push(format!(
        "INSERT INTO categories (id, name) VALUES {}",
        categories
            .iter()
            .enumerate()
            .map(|(i, name)| format!("({}, '{name}')", i + 1))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let products = [
        ("Ethiopia Yirgacheffe 250g", 1, 14.50),
        ("Colombia Huila 1kg", 1, 39.00),
        ("Kenya AA 250g", 1, 16.90),
        ("Espresso Blend 1kg", 1, 32.00),
        ("Decaf Swiss Water 250g", 1, 13.90),
        ("Sencha Uji 100g", 2, 18.00),
        ("Darjeeling First Flush", 2, 22.50),
        ("Hojicha 80g", 2, 11.00),
        ("V60 Dripper", 3, 24.00),
        ("Gooseneck Kettle", 3, 69.00),
        ("Burr Grinder", 3, 149.00),
        ("AeroPress Go", 3, 39.95),
        ("Paper Filters (100)", 4, 6.50),
        ("Glass Server 600ml", 4, 21.00),
        ("Scale with Timer", 4, 45.00),
        ("Gift Card 50", 5, 50.00),
    ];
    sql.push(format!(
        "INSERT INTO products (id, sku, name, category_id, price, stock, discontinued_at) VALUES {}",
        products
            .iter()
            .enumerate()
            .map(|(i, (name, category, price))| {
                let discontinued = if i == 4 { "'2025-11-30'" } else { "NULL" };
                format!(
                    "({}, 'SKU-{:04}', '{name}', {category}, {price:.2}, {}, {discontinued})",
                    i + 1,
                    1000 + i * 7,
                    (i * 37 + 11) % 180
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let first = [
        "Ada",
        "Grace",
        "Linus",
        "Margaret",
        "Ken",
        "Barbara",
        "Dennis",
        "Frances",
        "Alan",
        "Katherine",
        "Edsger",
        "Radia",
        "Guido",
        "Hedy",
        "Donald",
        "Sophie",
        "Tim",
        "Annie",
        "Bjarne",
        "Lynn",
        "John",
        "Jean",
        "Niklaus",
        "Mary",
    ];
    let last = [
        "Lovelace",
        "Hopper",
        "Torvalds",
        "Hamilton",
        "Thompson",
        "Liskov",
        "Ritchie",
        "Allen",
        "Turing",
        "Johnson",
        "Dijkstra",
        "Perlman",
        "van Rossum",
        "Lamarr",
        "Knuth",
        "Wilson",
        "Berners-Lee",
        "Easley",
        "Stroustrup",
        "Conway",
        "Backus",
        "Bartik",
        "Wirth",
        "Keller",
    ];
    let countries = ["FR", "US", "DE", "GB", "JP", "BE", "CA", "NL", "SE", "ES"];
    let mut rng = 0x2545F4914F6CDD1Du64;
    let mut next = move |bound: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % bound
    };
    let customers: Vec<String> = (0..first.len())
        .map(|i| {
            let name = format!("{} {}", first[i], last[i]);
            let email = format!(
                "{}.{}@example.com",
                first[i].to_lowercase(),
                last[i].to_lowercase().replace(' ', "")
            );
            format!(
                "({}, '{name}', '{email}', '{}', '2024-{:02}-{:02}', {})",
                i + 1,
                countries[i % countries.len()],
                1 + (i % 12),
                1 + (i * 3 % 27),
                i % 3 == 0
            )
            .replace("true", "1")
            .replace("false", "0")
        })
        .collect();
    sql.push(format!(
        "INSERT INTO customers (id, name, email, country, created_at, marketing_opt_in) VALUES {}",
        customers.join(", ")
    ));

    let statuses = [
        "delivered",
        "delivered",
        "delivered",
        "shipped",
        "paid",
        "pending",
        "cancelled",
    ];
    let mut orders = Vec::new();
    let mut items = Vec::new();
    for order_id in 1..=240u64 {
        let customer = 1 + next(first.len() as u64);
        let status = statuses[next(statuses.len() as u64) as usize];
        let month = 1 + next(12);
        let day = 1 + next(28);
        let ordered_at = format!(
            "2025-{month:02}-{day:02} {:02}:{:02}:00",
            8 + next(12),
            next(60)
        );
        let shipped = if matches!(status, "shipped" | "delivered") {
            format!("'2025-{month:02}-{:02}'", (day + 2).min(28))
        } else {
            "NULL".to_string()
        };
        let notes = match next(9) {
            0 => "'Gift wrap please'",
            1 => "'Leave at the door'",
            _ => "NULL",
        };
        orders.push(format!(
            "({order_id}, {customer}, '{status}', '{ordered_at}', {shipped}, {notes})"
        ));
        let mut used = Vec::new();
        for _ in 0..(1 + next(4)) {
            let product = 1 + next(products.len() as u64) as usize;
            if used.contains(&product) {
                continue;
            }
            used.push(product);
            items.push(format!(
                "({order_id}, {product}, {}, {:.2})",
                1 + next(3),
                products[product - 1].2
            ));
        }
    }
    sql.push(format!(
        "INSERT INTO orders (id, customer_id, status, ordered_at, shipped_at, notes) VALUES {}",
        orders.join(", ")
    ));
    sql.push(format!(
        "INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES {}",
        items.join(", ")
    ));
    sql
}

/// The Connections page in its other states.
fn home_variants(shots: &mut Shots<'_>, workspace: &Entity<dbcrust_gui::workspace::Workspace>) {
    use dbcrust_gui::app_state::{AttemptKind, AttemptSpec};
    use dbcrust_gui_core::launcher::{self, DockerListState, DockerProblemKind, Filter};

    let home = shots
        .cx
        .update(|cx| workspace.read(cx).connections_view())
        .expect("connections page");
    let window = shots.window;
    let only = shots.args.only.clone().unwrap_or_default();

    let start = |shots: &mut Shots<'_>, target: &str, subject: &str, url: &str| {
        let (target, subject, url) = (target.to_string(), subject.to_string(), url.to_string());
        shots.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.start_attempt(
                    AttemptSpec {
                        kind: AttemptKind::Connect,
                        target,
                        subject,
                        request: ConnectRequest::url(url),
                        identity: None,
                        environment: None,
                    },
                    cx,
                );
            })
        });
    };

    match only.as_str() {
        "home-new" => {
            shots
                .cx
                .update_window(window, |_, window, cx| {
                    home.update(cx, |home, cx| home.open_new_form(window, cx))
                })
                .ok();
        }
        "home-first-run" => {}
        "home-light" | "home-light-new" => {
            set_theme(shots.cx, ThemeMode::Light);
            if only == "home-light-new" {
                shots
                    .cx
                    .update_window(window, |_, window, cx| {
                        home.update(cx, |home, cx| home.open_new_form(window, cx))
                    })
                    .ok();
            }
        }
        // Run with `--size 960x600`: the smallest window the app allows
        "home-narrow" | "home-narrow-new" | "home-narrow-docker" => {
            shots
                .cx
                .update_window(window, |_, window, cx| {
                    home.update(cx, |home, cx| match only.as_str() {
                        "home-narrow-new" => home.open_new_form(window, cx),
                        "home-narrow-docker" => {
                            home.set_filter(Filter::Docker, cx);
                            home.select(&launcher::docker_key("billing-postgres-1"), cx);
                        }
                        _ => {}
                    });
                })
                .ok();
        }
        "home-engine-menu" => {
            shots
                .cx
                .update_window(window, |_, window, cx| {
                    home.update(cx, |home, cx| home.preview_engine_picker(window, cx));
                })
                .ok();
            // Keys go through the frame that was rendered last: draw the form first
            shots.settle(Duration::from_millis(300));
            shots
                .cx
                .update_window(window, |_, window, cx| window.press("down", cx))
                .ok();
        }
        "home-docker" => {
            shots.cx.update(|cx| {
                home.update(cx, |home, cx| {
                    home.set_filter(Filter::Docker, cx);
                    home.select(&launcher::docker_key("billing-postgres-1"), cx);
                })
            });
        }
        "home-docker-published" => {
            shots.cx.update(|cx| {
                home.update(cx, |home, cx| {
                    home.set_filter(Filter::Docker, cx);
                    home.select(&launcher::docker_key("shop-postgres"), cx);
                })
            });
        }
        "home-edit" => {
            shots
                .cx
                .update_window(window, |_, window, cx| {
                    home.update(cx, |home, cx| {
                        home.select(&launcher::session_key("prod-analytics"), cx);
                        home.edit_selected(window, cx);
                    })
                })
                .ok();
        }
        "home-docker-loading" | "home-docker-empty" | "home-docker-error" => {
            let state = match only.as_str() {
                "home-docker-loading" => DockerListState::Loading,
                "home-docker-empty" => DockerListState::Ready {
                    containers: Vec::new(),
                    context: Some("orbstack".into()),
                },
                _ => DockerListState::Problem {
                    kind: DockerProblemKind::Unavailable,
                    message: "Cannot connect to the Docker daemon at \
                              unix:///Users/demo/.orbstack/run/docker.sock. Is it running?"
                        .into(),
                    context: Some("orbstack".into()),
                },
            };
            shots.cx.update(|cx| {
                home.update(cx, |home, cx| {
                    home.preview_docker(state, cx);
                    home.set_filter(Filter::Docker, cx);
                })
            });
        }
        "home-docker-failed" | "home-docker-phases" => {
            use dbcrust_gui::app_state::AttemptState;
            use dbcrust_gui_core::docker::{DockerFailure, RouteAttempt, RouteKind};
            use dbcrust_gui_core::launcher::ConnectPhase;
            shots.cx.update(|cx| {
                home.update(cx, |home, cx| {
                    home.set_filter(Filter::Docker, cx);
                    home.select(&launcher::docker_key("billing-postgres-1"), cx);
                });
                let spec = || AttemptSpec {
                    kind: AttemptKind::Connect,
                    target: launcher::docker_key("billing-postgres-1"),
                    subject: "billing-postgres-1".into(),
                    request: ConnectRequest::url("docker://billing-postgres-1"),
                    identity: None,
                    environment: None,
                };
                AppState::global(cx).update(cx, |state, cx| {
                    if only == "home-docker-phases" {
                        state.preview_attempt(
                            spec(),
                            AttemptState::Running,
                            &[
                                (ConnectPhase::Starting, Duration::ZERO),
                                (ConnectPhase::ResolvingDocker, Duration::from_millis(40)),
                                (
                                    ConnectPhase::CheckingDirectAccess,
                                    Duration::from_millis(180),
                                ),
                                (
                                    ConnectPhase::OpeningForwarding,
                                    Duration::from_millis(2_200),
                                ),
                            ],
                            Duration::from_millis(3_400),
                            cx,
                        );
                    } else {
                        let failure = DockerFailure {
                            container: "billing-postgres-1".into(),
                            port: 5432,
                            headline: "Couldn't reach 'billing-postgres-1'".into(),
                            explanation: "Port 5432 isn't published, and the container address \
                                didn't respond. A VPN may be blocking that route. This image has \
                                no supported forwarding helper."
                                .into(),
                            attempts: vec![
                                RouteAttempt {
                                    route: RouteKind::ContainerIp,
                                    target: "192.168.107.3:5432".into(),
                                    reason: "no answer within 1.5 s".into(),
                                },
                                RouteAttempt {
                                    route: RouteKind::OrbstackDomain,
                                    target: "postgres.billing.orb.local:5432".into(),
                                    reason: "no answer within 1.5 s".into(),
                                },
                                RouteAttempt {
                                    route: RouteKind::ExecTunnel,
                                    target: "through Docker (docker exec)".into(),
                                    reason: "this image has no supported forwarding helper \
                                        (needs socat, perl or nc)"
                                        .into(),
                                },
                            ],
                            compose_example: "ports: [\"127.0.0.1::5432\"]".into(),
                            compose_note: "Applying it recreates the container; the port is then \
                                reachable on this computer only."
                                .into(),
                            message: String::new(),
                        };
                        state.preview_attempt(
                            spec(),
                            AttemptState::Failed(dbcrust_gui::app_state::AttemptFailure {
                                message: failure.explanation.clone(),
                                docker: Some(failure),
                            }),
                            &[],
                            Duration::from_secs(10),
                            cx,
                        );
                    }
                });
            });
        }
        "home-failed" => {
            shots.cx.update(|cx| {
                home.update(cx, |home, cx| {
                    home.select(&launcher::session_key("prod-analytics"), cx)
                })
            });
            // Nothing listens on port 1: the refusal is immediate
            start(
                shots,
                &launcher::session_key("prod-analytics"),
                "prod-analytics",
                "postgres://analyst@127.0.0.1:1/analytics",
            );
            shots.wait_until(Duration::from_secs(60), |cx| {
                let state = AppState::global(cx).read(cx);
                state
                    .attempt_for(&launcher::session_key("prod-analytics"))
                    .is_some_and(|attempt| !attempt.is_running())
            });
        }
        "home-connecting" => {
            shots.cx.update(|cx| {
                home.update(cx, |home, cx| {
                    home.select(&launcher::session_key("staging-api"), cx)
                })
            });
            // A black-hole address keeps the attempt running
            start(
                shots,
                &launcher::session_key("staging-api"),
                "staging-api",
                "postgres://api@10.255.255.1:5432/orders?connect_timeout=30",
            );
            shots.settle(Duration::from_millis(1800));
        }
        _ => {}
    }
    shots.settle(Duration::from_millis(900));
    shots.capture(&only);
}
