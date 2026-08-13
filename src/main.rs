#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anthropic;
mod auth;
mod commands;
mod db;
mod documents;
mod error;
mod markdown;
mod models;
mod prompts;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::Manager;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub db_path: PathBuf,
    pub http: reqwest::Client,
    /// In-flight generations: gen_id -> cancellation sender.
    pub gens: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("waypoint.db");
            let conn = db::open(&db_path)?;
            let http = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(20))
                .build()?;
            app.manage(AppState {
                db: Mutex::new(conn),
                db_path,
                http,
                gens: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_auth_status,
            commands::set_api_key,
            commands::clear_api_key,
            commands::test_api_key,
            commands::get_settings,
            commands::update_settings,
            commands::list_topics,
            commands::create_topic,
            commands::get_topic,
            commands::rename_topic,
            commands::delete_topic,
            commands::duplicate_topic,
            commands::set_topic_status,
            commands::reorder_topics,
            commands::advance_spine,
            commands::regenerate_step,
            commands::edit_step,
            commands::cancel_generation,
            commands::create_side_note,
            commands::reply_side_note,
            commands::retry_side_note,
            commands::set_side_note_resolved,
            commands::delete_side_note,
            commands::update_side_note_anchor,
            commands::export_topic_markdown,
            commands::generate_quiz,
            commands::import_documents,
            commands::read_dropped_files,
            commands::backup_database,
            commands::restore_database,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Waypoint");
}
