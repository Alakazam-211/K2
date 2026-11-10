// TEST ONLY: a thin Tauri shell over a web App (the window loads the
// App's page), the shape K2's Tauri template is made for.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default().run(tauri::generate_context!()).expect("the Tauri app");
}
