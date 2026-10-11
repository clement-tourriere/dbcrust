//! Context menus of the explorer rows.

use super::rows::{Row, RowKind};
use super::{Explorer, Generate, copy_text, dialogs};
use crate::app_state::AppState;
use dbcrust_gui_core::schema;
use dbcrust_gui_core::{ConnectionId, DatabaseType, TableEntry};
use gpui_kit::component::{
    IconName,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::*;

pub(super) fn build(
    menu: PopupMenu,
    row: &Row,
    explorer: WeakEntity<Explorer>,
    _window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    match &row.kind {
        RowKind::Connection => connection_menu(menu, row.connection, explorer, cx),
        RowKind::Table { table, .. } => {
            table_menu(menu, row.connection, table.entry().clone(), explorer, cx)
        }
        RowKind::Column { table, name, .. } => {
            let table_name = table.entry().name.clone();
            let column = name.to_string();
            let qualified = format!("{table_name}.{column}");
            menu.item(entry("Copy column name", IconName::Copy, {
                let column = column.clone();
                move |window, cx| copy_text(column.clone(), "column name", window, cx)
            }))
            .item(entry(
                "Copy table.column",
                IconName::Copy,
                move |window, cx| copy_text(qualified.clone(), "table.column", window, cx),
            ))
        }
        RowKind::SchemaFailed { .. } => menu.item(entry("Retry", IconName::RefreshCw, {
            let id = row.connection;
            move |_, cx| refresh(id, cx)
        })),
        _ => menu,
    }
}

fn entry(
    label: &'static str,
    icon: IconName,
    handler: impl Fn(&mut Window, &mut App) + 'static,
) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .icon(icon)
        .on_click(move |_, window, cx| handler(window, cx))
}

fn refresh(connection: ConnectionId, cx: &mut App) {
    AppState::global(cx).update(cx, |state, cx| state.refresh_schema(connection, cx));
}

fn connection_menu(
    menu: PopupMenu,
    id: ConnectionId,
    explorer: WeakEntity<Explorer>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (read_only, can_switch) = AppState::global(cx)
        .read(cx)
        .connection(id)
        .map(|live| {
            (
                live.summary.read_only,
                live.summary.supports_database_switch(),
            )
        })
        .unwrap_or_default();

    let mut menu = menu
        .item(entry(
            "New query",
            IconName::SquareTerminal,
            move |_, cx| {
                AppState::global(cx).update(cx, |state, cx| {
                    state.open_query(id, None, String::new(), false, cx)
                });
            },
        ))
        .item(entry(
            "Refresh schema",
            IconName::RefreshCw,
            move |_, cx| refresh(id, cx),
        ));
    if can_switch {
        let explorer = explorer.clone();
        menu = menu.item(entry(
            "Open database…",
            IconName::HardDrive,
            move |window, cx| {
                let _ = explorer.update(cx, |this, cx| this.pick_database(id, window, cx));
            },
        ));
    }
    menu.separator()
        .item(entry(
            "Save as session…",
            IconName::Star,
            move |window, cx| dialogs::open_session_dialog(id, window, cx),
        ))
        .item(
            PopupMenuItem::new("Read-only guard")
                .checked(read_only)
                .on_click(move |_, _, cx| {
                    AppState::global(cx).update(cx, |state, cx| {
                        state.set_read_only_guard(id, !read_only, cx)
                    });
                }),
        )
        .separator()
        .item(entry("Disconnect", IconName::Close, move |_, cx| {
            // Asks first when a query is still running on it
            crate::tray::request_disconnect(vec![id], cx);
        }))
}

fn table_menu(
    menu: PopupMenu,
    id: ConnectionId,
    table: TableEntry,
    explorer: WeakEntity<Explorer>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let db_type: DatabaseType = AppState::global(cx)
        .read(cx)
        .connection(id)
        .map(|live| live.summary.db_type.clone())
        .unwrap_or(DatabaseType::SQLite);
    let can_insert = schema::supports_insert(&db_type);
    let can_ddl = schema::supports_ddl(&db_type);
    let kind_label = table.kind.label().to_string();

    let select_sql = schema::select_star_sql(&db_type, &table, schema::PREVIEW_ROWS);
    let count_sql = schema::count_sql(&db_type, &table);

    let open_entry = table.clone();
    let select_table = table.clone();
    let count_table = table.clone();
    let name = table.name.clone();
    let qualified = table.qualified_name();

    let mut menu = menu
        .item(entry(
            match kind_label.as_str() {
                "view" | "materialized view" => "Open view",
                "collection" => "Open collection",
                "index" => "Open index",
                _ => "Open table",
            },
            IconName::ExternalLink,
            move |_, cx| {
                AppState::global(cx)
                    .update(cx, |state, cx| state.open_table(id, open_entry.clone(), cx));
            },
        ))
        .item(entry("Select top 100", IconName::Play, move |_, cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.open_query(
                    id,
                    Some(select_table.name.clone()),
                    select_sql.clone(),
                    true,
                    cx,
                )
            });
        }))
        .item(entry("Count rows", IconName::Asterisk, move |_, cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.open_query(
                    id,
                    Some(format!("count {}", count_table.name)),
                    count_sql.clone(),
                    true,
                    cx,
                )
            });
        }))
        .separator()
        .item(entry("Copy name", IconName::Copy, move |window, cx| {
            copy_text(name.clone(), "name", window, cx)
        }))
        .item(entry(
            "Copy qualified name",
            IconName::Copy,
            move |window, cx| copy_text(qualified.clone(), "qualified name", window, cx),
        ))
        .separator();

    let generate = |kind: Generate, table: TableEntry, explorer: WeakEntity<Explorer>| {
        move |window: &mut Window, cx: &mut App| {
            let table = table.clone();
            let _ = explorer.update(cx, |this, cx| this.generate(id, table, kind, window, cx));
        }
    };
    menu = menu.item(entry(
        "Generate SELECT",
        IconName::FileText,
        generate(Generate::Select, table.clone(), explorer.clone()),
    ));
    if can_insert {
        menu = menu.item(entry(
            "Generate INSERT",
            IconName::FileText,
            generate(Generate::Insert, table.clone(), explorer.clone()),
        ));
    }
    if can_ddl {
        menu = menu.item(entry(
            "Copy DDL",
            IconName::Copy,
            generate(Generate::Ddl, table, explorer),
        ));
    }
    menu
}
