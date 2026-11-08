mod commands;
mod qpx;

pub use commands::s3stream::{register as stream_register, register_http as stream_register_http, stats as stream_stats};

pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::sources::open_file,
            commands::sources::scan_folder,
            commands::sources::scan_collection,
            commands::io::read_file_bytes,
            commands::io::write_file_bytes,
            commands::s3::list_s3,
            commands::s3::cache_s3_views,
            commands::s3::presign_s3_urls,
            commands::s3stream::stream_s3_urls,
            commands::s3stream::stream_s3_stats,
            commands::s3stream::stream_http_urls,
            commands::s3stream::stream_local_files,
            commands::transfers::cancel_download,
            commands::transfers::cache_free_space,
            commands::portal::portal_datasets,
            commands::portal::portal_registry,
            commands::portal::portal_search,
            commands::portal::portal_facets,
            commands::portal::portal_list_views,
            commands::portal::portal_cache_views,
            commands::peptide_search::ps_search,
            commands::peptide_search::ps_modifications,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
