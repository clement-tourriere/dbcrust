//! Every user command of the app as a GPUI action. Key bindings, the
//! native menus and the command palette are all derived from
//! [`AppCommand`] (see `commands.rs`) so they never drift apart.
//!
//! Views register handlers with `.on_action(cx.listener(...))`; actions
//! bubble from the focused element up to the workspace.

use gpui_kit::actions;

actions!(
    dbcrust,
    [
        // Query editor
        RunStatement,
        RunAll,
        ExplainStatement,
        ExplainAnalyze,
        CancelQuery,
        SaveNamedQuery,
        OpenSqlFile,
        SaveSqlFile,
        FormatSql,
        // Tabs
        NewQuery,
        CloseTab,
        ReopenClosedTab,
        NextTab,
        PreviousTab,
        SwitchTab,
        RenameTab,
        GoToTab1,
        GoToTab2,
        GoToTab3,
        GoToTab4,
        GoToTab5,
        GoToTab6,
        GoToTab7,
        GoToTab8,
        GoToTab9,
        // Connections & schema
        NewConnection,
        Disconnect,
        RefreshSchema,
        ToggleReadOnly,
        SaveSession,
        // Results
        CopySelection,
        ExportResults,
        // Navigation
        ShowHome,
        OpenSettings,
        CommandPalette,
        FocusExplorer,
        ToggleSidebar,
        ToggleTheme,
        // App
        About,
        Quit,
    ]
);
