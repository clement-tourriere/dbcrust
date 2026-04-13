// DBCrust GUI — Desktop application entry point
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    dbcrust_gui_lib::run()
}
