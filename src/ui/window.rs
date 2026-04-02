use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::{ModelRc, VecModel};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::app::{HostKeyStatus, SharedState, SshCommand, SshEvent};
use crate::config::Settings;
use crate::models::connection::{AuthMethod, ConnectionProfile};
use crate::models::tunnel::TunnelConfig;
use crate::ssh::sftp::{SftpCommand, SftpConflictResponse, SftpEntry, SftpEvent};
use crate::ui::connection_list;
use crate::ui::dialogs;
use crate::ui::sftp;
use crate::ui::terminal::TerminalRenderer;

slint::include_modules!();

/// Represents an active session (terminal or SFTP).
struct SessionHandle {
    id: Uuid,
    title: String,
    is_terminal: bool,
    // Terminal-specific
    cmd_tx: Option<async_channel::Sender<SshCommand>>,
    renderer: Option<RefCell<TerminalRenderer>>,
    font_size: f32,
    // SFTP-specific
    sftp_cmd_tx: Option<async_channel::Sender<SftpCommand>>,
    remote_path: RefCell<String>,
    remote_entries: RefCell<Vec<SftpEntry>>,
    local_path: RefCell<PathBuf>,
}

/// Shared application state for the UI.
struct AppUiState {
    sessions: Vec<SessionHandle>,
    active_tab: usize,
    pending_tunnels: Vec<TunnelConfig>,
    editing_profile_id: Option<Uuid>,
    editing_profile_created_at: Option<i64>,
}

fn refresh_local_sftp_view(ui: &MainWindow, session: &SessionHandle) {
    let local_path = session.local_path.borrow().clone();
    let items = sftp::read_local_dir(&local_path);
    let selected = ui.get_sftp_local_selected();
    ui.set_sftp_local_path(local_path.display().to_string().into());
    ui.set_sftp_local_files(ModelRc::new(VecModel::from(items.clone())));
    ui.set_sftp_local_summary(sftp::selection_summary(&items, selected).into());
}

fn refresh_remote_sftp_view(ui: &MainWindow, session: &SessionHandle) {
    let remote_path = session.remote_path.borrow().clone();
    let entries = session.remote_entries.borrow().clone();
    let items = sftp::sftp_entries_to_items(&entries);
    let selected = ui.get_sftp_remote_selected();
    ui.set_sftp_remote_path(remote_path.into());
    ui.set_sftp_remote_files(ModelRc::new(VecModel::from(items.clone())));
    ui.set_sftp_remote_summary(sftp::selection_summary(&items, selected).into());
}

fn prompt_for_name(
    title: &str,
    body: &str,
    confirm_text: &str,
    initial_value: &str,
    on_confirm: impl Fn(String) + 'static,
) {
    let dialog = InputPromptDialog::new().unwrap();
    dialog.set_prompt_title(title.into());
    dialog.set_prompt_body(body.into());
    dialog.set_confirm_text(confirm_text.into());
    dialog.set_input_value(initial_value.into());

    let dialog_weak = dialog.as_weak();
    dialog.on_confirm_clicked(move || {
        if let Some(dialog) = dialog_weak.upgrade() {
            let value = dialog.get_input_value().trim().to_string();
            if !value.is_empty() {
                on_confirm(value);
                dialog.hide().ok();
            }
        }
    });

    let dialog_weak = dialog.as_weak();
    dialog.on_cancel_clicked(move || {
        if let Some(dialog) = dialog_weak.upgrade() {
            dialog.hide().ok();
        }
    });

    dialog.show().ok();
}

pub fn setup(ui: &MainWindow, state: SharedState) {
    let app_state = Rc::new(RefCell::new(AppUiState {
        sessions: Vec::new(),
        active_tab: 0,
        pending_tunnels: Vec::new(),
        editing_profile_id: None,
        editing_profile_created_at: None,
    }));

    {
        let settings = state.settings.lock().unwrap().clone();
        apply_ui_typography(ui, &settings);
    }

    // Initial connection list
    refresh_connection_list(ui, &state);

    // ── Sidebar callbacks ──

    // Add connection
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        let app_state = app_state.clone();
        ui.on_add_connection_clicked(move || {
            if let Some(ui) = ui_handle.upgrade() {
                show_connection_dialog(&ui, &state, &app_state, None);
            }
        });
    }

    // Connection search
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        ui.on_connection_search_edited(move |_| {
            if let Some(ui) = ui_handle.upgrade() {
                refresh_connection_list(&ui, &state);
            }
        });
    }

    // Connect (SSH terminal)
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        let app_state = app_state.clone();
        ui.on_connect_clicked(move |idx| {
            if let Some(ui) = ui_handle.upgrade() {
                if let Some(profile) = connection_list::get_profile_by_index(&state, idx as usize) {
                    start_ssh_session(&ui, &state, &app_state, &profile);
                }
            }
        });
    }

    // SFTP
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        let app_state = app_state.clone();
        ui.on_sftp_clicked(move |idx| {
            if let Some(ui) = ui_handle.upgrade() {
                if let Some(profile) = connection_list::get_profile_by_index(&state, idx as usize) {
                    start_sftp_session(&ui, &state, &app_state, &profile);
                }
            }
        });
    }

    // Edit connection
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        let app_state = app_state.clone();
        ui.on_edit_clicked(move |idx| {
            if let Some(ui) = ui_handle.upgrade() {
                if let Some(profile) = connection_list::get_profile_by_index(&state, idx as usize) {
                    show_connection_dialog(&ui, &state, &app_state, Some(profile));
                }
            }
        });
    }

    // Delete connection
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        ui.on_delete_clicked(move |idx| {
            dialogs::delete_connection(&state, idx as usize);
            if let Some(ui) = ui_handle.upgrade() {
                refresh_connection_list(&ui, &state);
            }
        });
    }

    // Key Manager
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        ui.on_key_manager_clicked(move || {
            if let Some(ui) = ui_handle.upgrade() {
                show_key_manager(&ui, &state);
            }
        });
    }

    // Preferences
    {
        let ui_handle = ui.as_weak();
        let state = state.clone();
        ui.on_preferences_clicked(move || {
            if let Some(ui) = ui_handle.upgrade() {
                show_preferences(&ui, &state);
            }
        });
    }

    // Backup connections
    {
        let state = state.clone();
        let _ui_handle = ui.as_weak();
        ui.on_backup_connections_clicked(move || {
            let store = state.profile_store.lock().unwrap();
            match store.export_backup() {
                Ok(json) => {
                    let dialog = rfd::FileDialog::new()
                        .set_title("Save Connections Backup")
                        .set_file_name("wrustyssh-connections-backup.json")
                        .add_filter("JSON", &["json"]);
                    if let Some(path) = dialog.save_file() {
                        if let Err(e) = std::fs::write(&path, &json) {
                            log::error!("Failed to write backup: {e}");
                        }
                    }
                }
                Err(e) => log::error!("Failed to export connections: {e}"),
            }
        });
    }

    // Restore connections
    {
        let state = state.clone();
        let ui_handle = ui.as_weak();
        ui.on_restore_connections_clicked(move || {
            let dialog = rfd::FileDialog::new()
                .set_title("Restore Connections from Backup")
                .add_filter("JSON", &["json"]);
            if let Some(path) = dialog.pick_file() {
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        let mut store = state.profile_store.lock().unwrap();
                        match store.import_backup(&json) {
                            Ok(count) => {
                                log::info!("Imported {count} connection(s).");
                                drop(store);
                                if let Some(ui) = ui_handle.upgrade() {
                                    refresh_connection_list(&ui, &state);
                                }
                            }
                            Err(e) => log::error!("Failed to import backup: {e}"),
                        }
                    }
                    Err(e) => log::error!("Failed to read backup file: {e}"),
                }
            }
        });
    }

    // ── Tab callbacks ──

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_tab_selected(move |idx| {
            let mut state = app_state.borrow_mut();
            state.active_tab = idx as usize;
            drop(state);

            if let Some(ui) = ui_handle.upgrade() {
                update_active_tab_content(&ui, &app_state);
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_tab_close(move |idx| {
            let idx = idx as usize;
            let mut state = app_state.borrow_mut();
            if idx < state.sessions.len() {
                // Send disconnect
                let session = &state.sessions[idx];
                if let Some(ref tx) = session.cmd_tx {
                    let tx = tx.clone();
                    let _ = tx.send_blocking(SshCommand::Disconnect);
                }
                if let Some(ref tx) = session.sftp_cmd_tx {
                    let tx = tx.clone();
                    let _ = tx.send_blocking(SftpCommand::Disconnect);
                }
                state.sessions.remove(idx);
                if state.active_tab >= state.sessions.len() && !state.sessions.is_empty() {
                    state.active_tab = state.sessions.len() - 1;
                }
            }
            drop(state);
            if let Some(ui) = ui_handle.upgrade() {
                refresh_tabs(&ui, &app_state);
                update_active_tab_content(&ui, &app_state);
            }
        });
    }

    // ── Terminal callbacks ──

    {
        let app_state = app_state.clone();
        ui.on_terminal_key_pressed(move |text, ctrl, shift, alt, meta| {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ref cmd_tx) = session.cmd_tx {
                    if let Some(data) =
                        crate::ui::terminal::translate_key(text.as_str(), ctrl, shift, alt, meta)
                    {
                        if let Some(renderer) = session.renderer.as_ref() {
                        if let Ok(mut renderer) = renderer.try_borrow_mut() {
                            renderer.clear_selection();
                            renderer.reset_viewport_to_bottom();
                        }
                        }
                        let tx = cmd_tx.clone();
                        let _ = tx.send_blocking(SshCommand::SendData(data));
                    }
                }
            }
        });
    }

    {
        let app_state = app_state.clone();
        ui.on_terminal_copy_requested(move || {
            let _ = copy_terminal_selection(&app_state);
        });
    }

    {
        let app_state = app_state.clone();
        ui.on_terminal_paste_requested(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ref cmd_tx) = session.cmd_tx {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            if let Some(renderer) = session.renderer.as_ref() {
                                if let Ok(mut renderer) = renderer.try_borrow_mut() {
                                    renderer.clear_selection();
                                    renderer.reset_viewport_to_bottom();
                                }
                            }
                            let tx = cmd_tx.clone();
                            let _ = tx.send_blocking(SshCommand::SendData(text.into_bytes()));
                        }
                    }
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_terminal_right_clicked(move || {
            if copy_terminal_selection(&app_state) {
                if let Some(ui) = ui_handle.upgrade() {
                    render_active_terminal(&ui, &app_state);
                }
                return;
            }

            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ref cmd_tx) = session.cmd_tx {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            if let Some(renderer) = session.renderer.as_ref() {
                                if let Ok(mut renderer) = renderer.try_borrow_mut() {
                                    renderer.clear_selection();
                                    renderer.reset_viewport_to_bottom();
                                }
                            }
                            let tx = cmd_tx.clone();
                            let _ = tx.send_blocking(SshCommand::SendData(text.into_bytes()));
                        }
                    }
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_terminal_selection_started(move |x, y| {
            {
                let state = app_state.borrow();
                if let Some(session) = state.sessions.get(state.active_tab) {
                    if let Some(renderer) = session.renderer.as_ref() {
                        if let Ok(mut renderer) = renderer.try_borrow_mut() {
                            renderer.begin_selection(x, y);
                        }
                    }
                }
            }
            if let Some(ui) = ui_handle.upgrade() {
                render_active_terminal(&ui, &app_state);
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_terminal_selection_updated(move |x, y| {
            {
                let state = app_state.borrow();
                if let Some(session) = state.sessions.get(state.active_tab) {
                    if let Some(renderer) = session.renderer.as_ref() {
                        if let Ok(mut renderer) = renderer.try_borrow_mut() {
                            renderer.update_selection(x, y);
                        }
                    }
                }
            }
            if let Some(ui) = ui_handle.upgrade() {
                render_active_terminal(&ui, &app_state);
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_terminal_scroll_requested(move |delta_y| {
            {
                let state = app_state.borrow();
                if let Some(session) = state.sessions.get(state.active_tab) {
                    if let Some(renderer) = session.renderer.as_ref() {
                        if let Ok(mut renderer) = renderer.try_borrow_mut() {
                            let delta_rows = if delta_y > 0.0 {
                                3
                            } else if delta_y < 0.0 {
                                -3
                            } else {
                                0
                            };
                            renderer.scroll_viewport(delta_rows);
                        }
                    }
                }
            }
            if let Some(ui) = ui_handle.upgrade() {
                render_active_terminal(&ui, &app_state);
            }
        });
    }

    // ── SFTP callbacks ──

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_navigate_up(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let current = session.local_path.borrow().clone();
                let parent = sftp::local_parent(&current);
                *session.local_path.borrow_mut() = parent.clone();
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_sftp_local_selected(-1);
                    refresh_local_sftp_view(&ui, session);
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_navigate_home(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let home = dirs_home();
                *session.local_path.borrow_mut() = home.clone();
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_sftp_local_selected(-1);
                    refresh_local_sftp_view(&ui, session);
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_navigate_to(move |path| {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let new_path = PathBuf::from(path.as_str());
                if new_path.is_dir() {
                    *session.local_path.borrow_mut() = new_path.clone();
                    if let Some(ui) = ui_handle.upgrade() {
                        ui.set_sftp_local_selected(-1);
                        refresh_local_sftp_view(&ui, session);
                    }
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_item_activated(move |idx| {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let current = session.local_path.borrow().clone();
                let items = sftp::read_local_dir(&current);
                if let Some(item) = items.get(idx as usize) {
                    if item.is_dir {
                        let new_path = current.join(item.name.as_str());
                        *session.local_path.borrow_mut() = new_path.clone();
                        if let Some(ui) = ui_handle.upgrade() {
                            ui.set_sftp_local_selected(-1);
                            refresh_local_sftp_view(&ui, session);
                        }
                    }
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_selection_changed(move |idx| {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let current = session.local_path.borrow().clone();
                let items = sftp::read_local_dir(&current);
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_sftp_local_summary(sftp::selection_summary(&items, idx).into());
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_refresh(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_sftp_local_selected(-1);
                    refresh_local_sftp_view(&ui, session);
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_new_folder(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let current = session.local_path.borrow().clone();
                drop(state);
                let ui_handle = ui_handle.clone();
                let app_state = app_state.clone();
                prompt_for_name(
                    "New Local Folder",
                    "Create a folder in the current local directory.",
                    "Create",
                    "New Folder",
                    move |name| {
                        let path = current.join(name);
                        if let Err(err) = std::fs::create_dir(&path) {
                            log::error!(
                                "Failed to create local folder {}: {}",
                                path.display(),
                                err
                            );
                            return;
                        }

                        let state = app_state.borrow();
                        if let Some(session) = state.sessions.get(state.active_tab) {
                            if let Some(ui) = ui_handle.upgrade() {
                                ui.set_sftp_local_selected(-1);
                                refresh_local_sftp_view(&ui, session);
                            }
                        }
                    },
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_local_rename(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let selected = if let Some(ui) = ui_handle.upgrade() {
                    ui.get_sftp_local_selected()
                } else {
                    -1
                };
                if selected < 0 {
                    return;
                }

                let current = session.local_path.borrow().clone();
                let items = sftp::read_local_dir(&current);
                let Some(item) = items.get(selected as usize) else {
                    return;
                };
                let current_item = current.join(item.name.as_str());
                let initial_name = item.name.to_string();
                drop(state);

                let ui_handle = ui_handle.clone();
                let app_state = app_state.clone();
                prompt_for_name(
                    "Rename Local Item",
                    "Enter a new name for the selected local item.",
                    "Rename",
                    &initial_name,
                    move |name| {
                        let target = current_item.parent().unwrap_or(&current_item).join(name);
                        if let Err(err) = std::fs::rename(&current_item, &target) {
                            log::error!(
                                "Failed to rename local item {} -> {}: {}",
                                current_item.display(),
                                target.display(),
                                err
                            );
                            return;
                        }

                        let state = app_state.borrow();
                        if let Some(session) = state.sessions.get(state.active_tab) {
                            if let Some(ui) = ui_handle.upgrade() {
                                ui.set_sftp_local_selected(-1);
                                refresh_local_sftp_view(&ui, session);
                            }
                        }
                    },
                );
            }
        });
    }

    // Remote SFTP navigation
    {
        let app_state = app_state.clone();
        ui.on_sftp_remote_navigate_up(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ref tx) = session.sftp_cmd_tx {
                    let current = session.remote_path.borrow().clone();
                    let parent = sftp::remote_parent(&current);
                    *session.remote_path.borrow_mut() = parent.clone();
                    let _ = tx.send_blocking(SftpCommand::ListDir(parent));
                }
            }
        });
    }

    {
        let app_state = app_state.clone();
        ui.on_sftp_remote_navigate_home(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ref tx) = session.sftp_cmd_tx {
                    *session.remote_path.borrow_mut() = ".".to_string();
                    let _ = tx.send_blocking(SftpCommand::ListDir(".".to_string()));
                }
            }
        });
    }

    {
        let app_state = app_state.clone();
        ui.on_sftp_remote_navigate_to(move |path| {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ref tx) = session.sftp_cmd_tx {
                    let path_str = path.to_string();
                    *session.remote_path.borrow_mut() = path_str.clone();
                    let _ = tx.send_blocking(SftpCommand::ListDir(path_str));
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_remote_item_activated(move |idx| {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let entries = session.remote_entries.borrow();
                if let Some(entry) = entries.get(idx as usize) {
                    if entry.is_dir {
                        let current = session.remote_path.borrow().clone();
                        let new_path = sftp::join_remote_child(&current, &entry.name);
                        *session.remote_path.borrow_mut() = new_path.clone();
                        if let Some(ref tx) = session.sftp_cmd_tx {
                            let _ = tx.send_blocking(SftpCommand::ListDir(new_path));
                        }
                        drop(entries);
                        if let Some(ui) = ui_handle.upgrade() {
                            ui.set_sftp_remote_selected(-1);
                            ui.set_sftp_remote_summary("No selection".into());
                        }
                    }
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_remote_selection_changed(move |idx| {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let entries = session.remote_entries.borrow().clone();
                let items = sftp::sftp_entries_to_items(&entries);
                if let Some(ui) = ui_handle.upgrade() {
                    ui.set_sftp_remote_summary(sftp::selection_summary(&items, idx).into());
                }
            }
        });
    }

    {
        let app_state = app_state.clone();
        ui.on_sftp_remote_refresh(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                if let Some(ref tx) = session.sftp_cmd_tx {
                    let path = session.remote_path.borrow().clone();
                    let _ = tx.send_blocking(SftpCommand::ListDir(path));
                }
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_remote_new_folder(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let Some(tx) = session.sftp_cmd_tx.as_ref().cloned() else {
                    return;
                };
                let current = session.remote_path.borrow().clone();
                drop(state);

                let ui_handle = ui_handle.clone();
                let app_state = app_state.clone();
                prompt_for_name(
                    "New Remote Folder",
                    "Create a folder on the remote server in the current directory.",
                    "Create",
                    "New Folder",
                    move |name| {
                        let new_dir = sftp::join_remote_child(&current, &name);
                        let _ = tx.send_blocking(SftpCommand::MkDir(new_dir));
                        let _ = tx.send_blocking(SftpCommand::ListDir(current.clone()));

                        let state = app_state.borrow();
                        if let Some(session) = state.sessions.get(state.active_tab) {
                            if let Some(ui) = ui_handle.upgrade() {
                                ui.set_sftp_remote_selected(-1);
                                refresh_remote_sftp_view(&ui, session);
                            }
                        }
                    },
                );
            }
        });
    }

    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_remote_rename(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let selected = if let Some(ui) = ui_handle.upgrade() {
                    ui.get_sftp_remote_selected()
                } else {
                    -1
                };
                if selected < 0 {
                    return;
                }

                let entries = session.remote_entries.borrow();
                let Some(entry) = entries.get(selected as usize) else {
                    return;
                };
                let current = session.remote_path.borrow().clone();
                let source = sftp::join_remote_child(&current, &entry.name);
                let initial_name = entry.name.clone();
                let Some(tx) = session.sftp_cmd_tx.as_ref().cloned() else {
                    return;
                };
                drop(entries);
                drop(state);

                prompt_for_name(
                    "Rename Remote Item",
                    "Enter a new name for the selected remote item.",
                    "Rename",
                    &initial_name,
                    move |name| {
                        let target = sftp::join_remote_child(&sftp::remote_parent(&source), &name);
                        let _ = tx.send_blocking(SftpCommand::Rename {
                            from: source.clone(),
                            to: target,
                        });
                        let _ = tx.send_blocking(SftpCommand::ListDir(current.clone()));
                    },
                );
            }
        });
    }

    // Upload
    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_upload_clicked(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let local_selected = if let Some(ui) = ui_handle.upgrade() {
                    ui.get_sftp_local_selected()
                } else {
                    -1
                };
                if local_selected >= 0 {
                    let local_path = session.local_path.borrow().clone();
                    let local_items = sftp::read_local_dir(&local_path);
                    if let Some(item) = local_items.get(local_selected as usize) {
                        let local_file = local_path.join(item.name.as_str());
                        let remote_path = session.remote_path.borrow().clone();
                        let remote_target =
                            sftp::join_remote_child(&remote_path, item.name.as_str());
                        if let Some(ref tx) = session.sftp_cmd_tx {
                            let _ = tx.send_blocking(SftpCommand::Upload {
                                local: local_file,
                                remote: remote_target,
                            });
                            // Refresh remote after upload
                            let _ = tx.send_blocking(SftpCommand::ListDir(remote_path));
                        }
                    }
                }
            }
        });
    }

    // Download
    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_download_clicked(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let remote_selected = if let Some(ui) = ui_handle.upgrade() {
                    ui.get_sftp_remote_selected()
                } else {
                    -1
                };
                if remote_selected >= 0 {
                    let entries = session.remote_entries.borrow();
                    if let Some(entry) = entries.get(remote_selected as usize) {
                        let remote_path = session.remote_path.borrow().clone();
                        let remote_file = sftp::join_remote_child(&remote_path, &entry.name);
                        let local_path = session.local_path.borrow().clone();
                        if let Some(ref tx) = session.sftp_cmd_tx {
                            let _ = tx.send_blocking(SftpCommand::Download {
                                remote: remote_file,
                                local: local_path.clone(),
                            });
                        }
                        drop(entries);
                        drop(state);
                        // Refresh local after download
                        if let Some(ui) = ui_handle.upgrade() {
                            let items = sftp::read_local_dir(&local_path);
                            ui.set_sftp_local_files(ModelRc::new(VecModel::from(items)));
                        }
                    }
                }
            }
        });
    }

    // Delete selected local entry
    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_delete_local_clicked(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let local_selected = if let Some(ui) = ui_handle.upgrade() {
                    ui.get_sftp_local_selected()
                } else {
                    -1
                };
                if local_selected >= 0 {
                    let local_path = session.local_path.borrow().clone();
                    let items = sftp::read_local_dir(&local_path);
                    if let Some(item) = items.get(local_selected as usize) {
                        let local_entry = local_path.join(item.name.as_str());
                        let result = if local_entry.is_dir() {
                            std::fs::remove_dir_all(&local_entry)
                        } else {
                            std::fs::remove_file(&local_entry)
                        };

                        if let Err(err) = result {
                            log::error!(
                                "Failed to delete local entry {}: {}",
                                local_entry.display(),
                                err
                            );
                            return;
                        }

                        if let Some(ui) = ui_handle.upgrade() {
                            ui.set_sftp_local_selected(-1);
                            refresh_local_sftp_view(&ui, session);
                        }
                    }
                }
            }
        });
    }

    // Delete selected remote entry
    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        ui.on_sftp_delete_remote_clicked(move || {
            let state = app_state.borrow();
            if let Some(session) = state.sessions.get(state.active_tab) {
                let remote_selected = if let Some(ui) = ui_handle.upgrade() {
                    ui.get_sftp_remote_selected()
                } else {
                    -1
                };
                if remote_selected >= 0 {
                    let entries = session.remote_entries.borrow();
                    if let Some(entry) = entries.get(remote_selected as usize) {
                        let remote_path = session.remote_path.borrow().clone();
                        let remote_file = sftp::join_remote_child(&remote_path, &entry.name);
                        if let Some(ref tx) = session.sftp_cmd_tx {
                            let _ = tx.send_blocking(SftpCommand::Remove(remote_file));
                            let _ = tx.send_blocking(SftpCommand::ListDir(remote_path));
                        }
                    }
                }
            }
        });
    }

    // ── Resize polling timer ──
    // Re-renders the terminal when the content area size changes (e.g. window resize)
    {
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        let last_size: Rc<RefCell<(i32, i32)>> = Rc::new(RefCell::new((0, 0)));
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                let Some(ui) = ui_handle.upgrade() else {
                    return;
                };
                let w = ui.get_content_area_width();
                let h = ui.get_content_area_height();
                let mut last = last_size.borrow_mut();
                if last.0 == w && last.1 == h {
                    return;
                }
                *last = (w, h);
                drop(last);

                let state = app_state.borrow();
                let active = state.active_tab;
                if let Some(session) = state.sessions.get(active) {
                    if !session.is_terminal {
                        return;
                    }
                    resize_terminal_session(session, w.max(100) as usize, h.max(100) as usize);
                    drop(state);
                    render_active_terminal(&ui, &app_state);
                }
            },
        );
        // Keep timer alive by leaking it (it runs for the app's lifetime)
        std::mem::forget(timer);
    }
}

fn dirs_home() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn terminal_settings(state: &SharedState) -> (String, f32, usize, String, String) {
    let settings = state.settings.lock().unwrap().clone();
    (
        settings.font_family,
        settings.font_size.max(13) as f32,
        settings.scrollback_lines.max(100) as usize,
        settings.default_terminal_type,
        settings.terminal_color_scheme,
    )
}

fn available_font_families() -> Vec<slint::SharedString> {
    let mut families: BTreeSet<String> = [
        "Segoe UI",
        "Bahnschrift",
        "Calibri",
        "Arial",
        "Consolas",
        "Cascadia Mono",
        "Cascadia Code",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();

    if let Ok(entries) = std::fs::read_dir(r"C:\Windows\Fonts") {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if !matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if let Some(name) = normalize_font_family_name(stem) {
                families.insert(name);
            }
        }
    }

    families.into_iter().map(Into::into).collect()
}

fn normalize_font_family_name(stem: &str) -> Option<String> {
    let lower = stem.to_ascii_lowercase();
    let mapped = if lower.starts_with("segoeui") || lower.starts_with("segui") {
        "Segoe UI".to_string()
    } else if lower.starts_with("bahnschrift") {
        "Bahnschrift".to_string()
    } else if lower.starts_with("calibri") {
        "Calibri".to_string()
    } else if lower.starts_with("arial") {
        "Arial".to_string()
    } else if lower.starts_with("consola") {
        "Consolas".to_string()
    } else if lower == "cascadiamono" {
        "Cascadia Mono".to_string()
    } else if lower == "cascadiacode" {
        "Cascadia Code".to_string()
    } else if lower
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == ' ')
    {
        stem.to_string()
    } else {
        return None;
    };
    Some(mapped)
}

fn font_index(fonts: &[slint::SharedString], name: &str) -> i32 {
    fonts
        .iter()
        .position(|font| font.as_str().eq_ignore_ascii_case(name))
        .unwrap_or(0) as i32
}

fn apply_ui_typography(ui: &MainWindow, settings: &Settings) {
    ui.set_app_font_family(settings.app_font_family.clone().into());
    ui.set_app_font_size(settings.app_font_size as i32);
    ui.set_button_font_size(settings.button_font_size as i32);
    ui.set_connection_name_font_size(settings.connection_name_font_size as i32);
    ui.set_connection_subtitle_font_size(settings.connection_subtitle_font_size as i32);
}

fn terminal_viewport_size(ui: &MainWindow) -> (usize, usize) {
    let width = (ui.get_content_area_width() - 24).max(100) as usize;
    let height = (ui.get_content_area_height() - 24).max(100) as usize;
    (width, height)
}

fn resize_terminal_session(session: &SessionHandle, width: usize, height: usize) {
    let Some(ref renderer) = session.renderer else {
        return;
    };
    let Ok(mut renderer) = renderer.try_borrow_mut() else {
        return;
    };
    let (cell_width, cell_height) = renderer.cell_size();
    if cell_width == 0 || cell_height == 0 {
        return;
    }

    let cols = (width / cell_width).max(10) as u16;
    let rows = (height / cell_height).max(2) as u16;
    let current = renderer.parser.screen().size();
    if current.0 == rows && current.1 == cols {
        return;
    }

    renderer.set_size(rows, cols);
    if let Some(ref cmd_tx) = session.cmd_tx {
        let _ = cmd_tx.send_blocking(SshCommand::Resize {
            cols: cols as u32,
            rows: rows as u32,
        });
    }
}

fn render_terminal_image(
    session: &SessionHandle,
    width: usize,
    height: usize,
) -> Option<slint::Image> {
    session.renderer.as_ref().and_then(|renderer| {
        renderer
            .try_borrow_mut()
            .ok()
            .map(|mut renderer| renderer.render_to_size(session.font_size, width, height))
    })
}

fn render_active_terminal(ui: &MainWindow, app_state: &Rc<RefCell<AppUiState>>) {
    let state = app_state.borrow();
    if let Some(session) = state.sessions.get(state.active_tab) {
        if !session.is_terminal {
            return;
        }

        let (width, height) = terminal_viewport_size(ui);
        resize_terminal_session(session, width, height);
        if let Some(image) = render_terminal_image(session, width, height) {
            ui.set_terminal_image(image);
        }
    }
}

fn copy_terminal_selection(app_state: &Rc<RefCell<AppUiState>>) -> bool {
    let state = app_state.borrow();
    let Some(session) = state.sessions.get(state.active_tab) else {
        return false;
    };
    let Some(renderer) = session.renderer.as_ref() else {
        return false;
    };
    let Ok(renderer) = renderer.try_borrow() else {
        return false;
    };
    let Some(text) = renderer.selected_text() else {
        return false;
    };
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if clipboard.set_text(text).is_ok() {
            return true;
        }
    }
    false
}

fn refresh_connection_list(ui: &MainWindow, state: &SharedState) {
    let items = connection_list::build_connection_items(state, ui.get_connection_search().as_str());
    ui.set_connections(ModelRc::new(VecModel::from(items)));
}

fn refresh_tabs(ui: &MainWindow, app_state: &Rc<RefCell<AppUiState>>) {
    let state = app_state.borrow();
    let tab_items: Vec<TabItem> = state
        .sessions
        .iter()
        .enumerate()
        .map(|(idx, s)| TabItem {
            id: s.id.to_string().into(),
            title: s.title.clone().into(),
            is_terminal: s.is_terminal,
            index: idx as i32,
        })
        .collect();
    ui.set_tabs(ModelRc::new(VecModel::from(tab_items)));
    ui.set_active_tab_index(state.active_tab as i32);
}

fn update_active_tab_content(ui: &MainWindow, app_state: &Rc<RefCell<AppUiState>>) {
    let state = app_state.borrow();
    if let Some(session) = state.sessions.get(state.active_tab) {
        ui.set_show_terminal(session.is_terminal);

        if session.is_terminal {
            let (width, height) = terminal_viewport_size(ui);
            resize_terminal_session(session, width, height);
            if let Some(image) = render_terminal_image(session, width, height) {
                ui.set_terminal_image(image);
            }
        } else {
            // Update SFTP view
            refresh_local_sftp_view(ui, session);
            refresh_remote_sftp_view(ui, session);
        }
    }
    ui.set_active_tab_index(state.active_tab as i32);
}

fn start_ssh_session(
    ui: &MainWindow,
    state: &SharedState,
    app_state: &Rc<RefCell<AppUiState>>,
    profile: &ConnectionProfile,
) {
    let needs_password = matches!(profile.auth_method, AuthMethod::Password | AuthMethod::Both);
    let key_has_passphrase = if let Some(key_id) = profile.key_pair_id {
        let store = state.key_store.lock().unwrap();
        store
            .get(&key_id)
            .map(|k| k.has_passphrase)
            .unwrap_or(false)
    } else {
        false
    };

    let ui_handle = ui.as_weak();
    let app_state = app_state.clone();
    let profile = profile.clone();

    if needs_password {
        let profile2 = profile.clone();
        let state2 = state.clone();
        let ui_handle2 = ui_handle.clone();
        let app_state2 = app_state.clone();
        prompt_password_then(&format!("Password for {}", profile.name), move |password| {
            if key_has_passphrase {
                let profile3 = profile2.clone();
                let state3 = state2.clone();
                prompt_password_then(
                    &format!("Key passphrase for {}", profile2.name),
                    move |key_passphrase| {
                        if let Some(ui) = ui_handle2.upgrade() {
                            do_start_ssh_session(
                                &ui,
                                &state3,
                                &app_state2,
                                &profile3,
                                password,
                                key_passphrase,
                            );
                        }
                    },
                );
            } else if let Some(ui) = ui_handle2.upgrade() {
                do_start_ssh_session(&ui, &state2, &app_state2, &profile2, password, None);
            }
        });
    } else if key_has_passphrase {
        let profile2 = profile.clone();
        let state2 = state.clone();
        prompt_password_then(
            &format!("Key passphrase for {}", profile.name),
            move |key_passphrase| {
                if let Some(ui) = ui_handle.upgrade() {
                    do_start_ssh_session(&ui, &state2, &app_state, &profile2, None, key_passphrase);
                }
            },
        );
    } else {
        do_start_ssh_session(ui, state, &app_state, &profile, None, None);
    }
}

/// Show a password prompt dialog and call `on_done` with the result.
/// Uses a callback-based approach since we can't block the Slint event loop.
fn prompt_password_then<F>(heading: &str, on_done: F)
where
    F: FnOnce(Option<Zeroizing<String>>) + 'static,
{
    let dialog = PasswordPrompt::new().unwrap();
    dialog.set_prompt_heading(heading.into());
    dialog.set_password_value("".into());

    let on_done = Rc::new(RefCell::new(Some(on_done)));

    {
        let dialog_handle = dialog.as_weak();
        let on_done = on_done.clone();
        dialog.on_submit_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                let val = d.get_password_value().to_string();
                d.hide().unwrap();
                if let Some(cb) = on_done.borrow_mut().take() {
                    if val.is_empty() {
                        cb(None);
                    } else {
                        cb(Some(Zeroizing::new(val)));
                    }
                }
            }
        });
    }

    {
        let dialog_handle = dialog.as_weak();
        let on_done = on_done.clone();
        dialog.on_cancel_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                d.hide().unwrap();
            }
            if let Some(cb) = on_done.borrow_mut().take() {
                cb(None);
            }
        });
    }

    dialog.show().unwrap();
}

fn show_host_key_dialog(
    hostname: &str,
    key_type: &str,
    fingerprint: &str,
    status: &HostKeyStatus,
    response_tx: async_channel::Sender<bool>,
    pending_data: &Rc<RefCell<Vec<Vec<u8>>>>,
) {
    let is_warning = matches!(status, HostKeyStatus::Changed { .. });

    let (title, body) = match status {
        HostKeyStatus::New => (
            "New Host Key".to_string(),
            format!(
                "The server at {hostname} presented a host key that is not in your known hosts.\n\n\
                 Key type: {key_type}\n\
                 Fingerprint: {fingerprint}\n\n\
                 Do you want to trust this host and continue connecting?"
            ),
        ),
        HostKeyStatus::Changed { old_fingerprint } => (
            "HOST KEY CHANGED".to_string(),
            format!(
                "WARNING: The host key for {hostname} has CHANGED!\n\n\
                 This could indicate a man-in-the-middle attack, or the server was reinstalled.\n\n\
                 Key type: {key_type}\n\
                 Old fingerprint: {old_fingerprint}\n\
                 New fingerprint: {fingerprint}\n\n\
                 Accepting will update your known hosts. Reject to abort the connection."
            ),
        ),
    };

    let dialog = HostKeyDialog::new().unwrap();
    dialog.set_dialog_title(title.into());
    dialog.set_dialog_body(body.into());
    dialog.set_is_warning(is_warning);

    let info_msg = format!(
        "\r\n[Host key ({key_type}): {fingerprint}]\r\n"
    );
    pending_data.borrow_mut().push(info_msg.into_bytes());

    let tx = response_tx.clone();
    let dialog_weak = dialog.as_weak();
    let pending = pending_data.clone();
    dialog.on_accept_clicked(move || {
        let _ = tx.send_blocking(true);
        pending
            .borrow_mut()
            .push("\r\n[Host key accepted]\r\n".as_bytes().to_vec());
        if let Some(d) = dialog_weak.upgrade() {
            d.hide().ok();
        }
    });

    let tx = response_tx;
    let dialog_weak = dialog.as_weak();
    let pending = pending_data.clone();
    dialog.on_reject_clicked(move || {
        let _ = tx.send_blocking(false);
        pending
            .borrow_mut()
            .push("\r\n[Host key rejected — connection aborted]\r\n".as_bytes().to_vec());
        if let Some(d) = dialog_weak.upgrade() {
            d.hide().ok();
        }
    });

    dialog.show().ok();
}

fn do_start_ssh_session(
    ui: &MainWindow,
    state: &SharedState,
    app_state: &Rc<RefCell<AppUiState>>,
    profile: &ConnectionProfile,
    password: Option<Zeroizing<String>>,
    key_passphrase: Option<Zeroizing<String>>,
) {
    let session_id = Uuid::new_v4();
    let (font_family, font_size, scrollback_lines, terminal_type, terminal_color_scheme) =
        terminal_settings(state);
    let (content_w, content_h) = terminal_viewport_size(ui);

    // Create a temporary renderer to get cell dimensions
    let temp_renderer = TerminalRenderer::new(
        80,
        24,
        &font_family,
        font_size,
        scrollback_lines,
        &terminal_color_scheme,
    );
    let (cw, ch) = temp_renderer.cell_size();
    drop(temp_renderer);

    let init_cols = if cw > 0 {
        (content_w / cw).max(20) as u16
    } else {
        80
    };
    let init_rows = if ch > 0 {
        (content_h / ch).max(5) as u16
    } else {
        24
    };

    log::info!(
        "Terminal init: content_area={}x{}, cell={}x{}, grid={}x{}",
        content_w,
        content_h,
        cw,
        ch,
        init_cols,
        init_rows
    );

    let (event_tx, event_rx) = async_channel::bounded::<SshEvent>(4096);
    let cmd_tx = crate::ssh::session::spawn_session(
        profile.clone(),
        password,
        key_passphrase,
        terminal_type,
        init_cols as u32,
        init_rows as u32,
        event_tx,
    );

    let renderer = TerminalRenderer::new(
        init_cols,
        init_rows,
        &font_family,
        font_size,
        scrollback_lines,
        &terminal_color_scheme,
    );

    // Note: tunnels are started inside run_session() after the SSH connection is established

    let session = SessionHandle {
        id: session_id,
        title: profile.name.clone(),
        is_terminal: true,
        cmd_tx: Some(cmd_tx),
        renderer: Some(RefCell::new(renderer)),
        font_size,
        sftp_cmd_tx: None,
        remote_path: RefCell::new(String::new()),
        remote_entries: RefCell::new(Vec::new()),
        local_path: RefCell::new(dirs_home()),
    };

    {
        let mut state = app_state.borrow_mut();
        state.sessions.push(session);
        state.active_tab = state.sessions.len() - 1;
    }

    refresh_tabs(ui, app_state);
    update_active_tab_content(ui, app_state);

    // Spawn async event loop for this session
    let ui_handle = ui.as_weak();
    let app_state_clone = app_state.clone();
    let tab_index = app_state.borrow().sessions.len() - 1;

    // Shared buffer: the async loop pushes raw data here; the render timer
    // drains and processes it right before painting so every frame sees a
    // complete terminal state (no half-drawn progress bars).
    let pending_data: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
    let session_alive = Rc::new(RefCell::new(true));

    // Fixed-interval render timer (~60 fps).  Processes ALL buffered data,
    // then renders once.  Because processing and painting happen atomically
    // within a single timer tick, the screen can never show a mid-update state.
    let render_timer = {
        let timer = slint::Timer::default();
        let ui_handle = ui_handle.clone();
        let app_state = app_state_clone.clone();
        let pending_data = pending_data.clone();
        let session_alive = session_alive.clone();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let mut pending = pending_data.borrow_mut();
                if pending.is_empty() {
                    return;
                }

                let Some(ui) = ui_handle.upgrade() else {
                    return;
                };
                let is_active = {
                    let state = app_state.borrow();
                    state
                        .sessions
                        .get(tab_index)
                        .map(|session| state.active_tab == tab_index && session.is_terminal)
                        .unwrap_or(false)
                };

                // Process buffered data through the terminal emulator
                {
                    let state = app_state.borrow();
                    if let Some(session) = state.sessions.get(tab_index) {
                        if let Some(ref renderer) = session.renderer {
                            if let Ok(mut r) = renderer.try_borrow_mut() {
                                for chunk in pending.drain(..) {
                                    r.process(&chunk);
                                }
                            }
                        }
                    }
                }
                pending.clear(); // in case renderer was None

                if is_active {
                    render_active_terminal(&ui, &app_state);
                }

                // Stop polling once the session is gone
                if !*session_alive.borrow() && pending_data.borrow().is_empty() {
                    // timer will be dropped when Rc cycle is broken
                }
            },
        );
        timer
    };

    slint::spawn_local(async move {
        let _render_timer = render_timer; // prevent drop until session ends
        loop {
            let event = match event_rx.recv().await {
                Ok(ev) => ev,
                Err(_) => break,
            };
            let ui_handle = ui_handle.clone();
            let app_state = app_state_clone.clone();

            match event {
                SshEvent::Connected => {
                    log::info!("SSH session connected");
                }
                SshEvent::Data(data) => {
                    // Just buffer the data — the render timer will process it.
                    // Also drain any additional Data events already queued so the
                    // buffer stays as current as possible.
                    let mut pending = pending_data.borrow_mut();
                    pending.push(data);
                    while let Ok(queued) = event_rx.try_recv() {
                        if let SshEvent::Data(more) = queued {
                            pending.push(more);
                        } else {
                            drop(pending);
                            // Re-handle non-data event inline
                            handle_non_data_event(
                                queued,
                                tab_index,
                                &app_state,
                                &ui_handle,
                                &pending_data,
                                &session_alive,
                            );
                            break;
                        }
                    }
                }
                SshEvent::Disconnected(reason) => {
                    let msg = if let Some(ref reason) = reason {
                        format!("\r\n[Disconnected: {}]\r\n", reason)
                    } else {
                        "\r\n[Disconnected]\r\n".to_string()
                    };
                    pending_data.borrow_mut().push(msg.into_bytes());
                    *session_alive.borrow_mut() = false;

                    // Force an immediate final render so the message is visible
                    if let Some(ui) = ui_handle.upgrade() {
                        flush_pending_data(tab_index, &app_state, &pending_data);
                        render_active_terminal(&ui, &app_state);
                    }
                    break;
                }
                SshEvent::Error(msg) => {
                    let err_msg = format!("\r\n[Error: {}]\r\n", msg);
                    pending_data.borrow_mut().push(err_msg.into_bytes());

                    if let Some(ui) = ui_handle.upgrade() {
                        flush_pending_data(tab_index, &app_state, &pending_data);
                        render_active_terminal(&ui, &app_state);
                    }
                }
                SshEvent::HostKeyVerify {
                    hostname,
                    key_type,
                    fingerprint,
                    status,
                    response_tx,
                } => {
                    show_host_key_dialog(
                        &hostname,
                        &key_type,
                        &fingerprint,
                        &status,
                        response_tx,
                        &pending_data,
                    );
                }
                SshEvent::TunnelEstablished(id) => {
                    let msg = format!("\r\n[Tunnel {} established]\r\n", id);
                    pending_data.borrow_mut().push(msg.into_bytes());
                }
                SshEvent::TunnelFailed(id, err) => {
                    let msg = format!("\r\n[Tunnel {} failed: {}]\r\n", id, err);
                    pending_data.borrow_mut().push(msg.into_bytes());
                }
            }
        }
    })
    .unwrap();
}

/// Process a non-Data event that was found while draining the event channel.
fn handle_non_data_event(
    event: SshEvent,
    _tab_index: usize,
    _app_state: &Rc<RefCell<AppUiState>>,
    _ui_handle: &slint::Weak<MainWindow>,
    pending_data: &Rc<RefCell<Vec<Vec<u8>>>>,
    _session_alive: &Rc<RefCell<bool>>,
) {
    // Push any text the event wants to display into the pending buffer so it
    // gets processed together with surrounding data in the render timer.
    match event {
        SshEvent::Data(_) => unreachable!(),
        SshEvent::Connected => {
            log::info!("SSH session connected (from drain)");
        }
        SshEvent::Disconnected(reason) => {
            let msg = if let Some(ref reason) = reason {
                format!("\r\n[Disconnected: {}]\r\n", reason)
            } else {
                "\r\n[Disconnected]\r\n".to_string()
            };
            pending_data.borrow_mut().push(msg.into_bytes());
            *_session_alive.borrow_mut() = false;
        }
        SshEvent::Error(msg) => {
            let err_msg = format!("\r\n[Error: {}]\r\n", msg);
            pending_data.borrow_mut().push(err_msg.into_bytes());
        }
        SshEvent::HostKeyVerify {
            hostname,
            key_type,
            fingerprint,
            status,
            response_tx,
        } => {
            show_host_key_dialog(
                &hostname,
                &key_type,
                &fingerprint,
                &status,
                response_tx,
                pending_data,
            );
        }
        SshEvent::TunnelEstablished(id) => {
            let msg = format!("\r\n[Tunnel {} established]\r\n", id);
            pending_data.borrow_mut().push(msg.into_bytes());
        }
        SshEvent::TunnelFailed(id, err) => {
            let msg = format!("\r\n[Tunnel {} failed: {}]\r\n", id, err);
            pending_data.borrow_mut().push(msg.into_bytes());
        }
    }
}

/// Drain the pending-data buffer through the terminal renderer immediately.
fn flush_pending_data(
    tab_index: usize,
    app_state: &Rc<RefCell<AppUiState>>,
    pending_data: &Rc<RefCell<Vec<Vec<u8>>>>,
) {
    let mut pending = pending_data.borrow_mut();
    if pending.is_empty() {
        return;
    }
    let state = app_state.borrow();
    if let Some(session) = state.sessions.get(tab_index) {
        if let Some(ref renderer) = session.renderer {
            if let Ok(mut r) = renderer.try_borrow_mut() {
                for chunk in pending.drain(..) {
                    r.process(&chunk);
                }
            }
        }
    }
    pending.clear();
}

fn start_sftp_session(
    ui: &MainWindow,
    state: &SharedState,
    app_state: &Rc<RefCell<AppUiState>>,
    profile: &ConnectionProfile,
) {
    let needs_password = matches!(profile.auth_method, AuthMethod::Password | AuthMethod::Both);
    let key_has_passphrase = if let Some(key_id) = profile.key_pair_id {
        let store = state.key_store.lock().unwrap();
        store
            .get(&key_id)
            .map(|k| k.has_passphrase)
            .unwrap_or(false)
    } else {
        false
    };

    let ui_handle = ui.as_weak();
    let app_state = app_state.clone();
    let profile = profile.clone();

    if needs_password {
        let profile2 = profile.clone();
        let ui_handle2 = ui_handle.clone();
        let app_state2 = app_state.clone();
        prompt_password_then(&format!("Password for {}", profile.name), move |password| {
            if key_has_passphrase {
                let profile3 = profile2.clone();
                prompt_password_then(
                    &format!("Key passphrase for {}", profile2.name),
                    move |key_passphrase| {
                        if let Some(ui) = ui_handle2.upgrade() {
                            do_start_sftp_session(
                                &ui,
                                &app_state2,
                                &profile3,
                                password,
                                key_passphrase,
                            );
                        }
                    },
                );
            } else if let Some(ui) = ui_handle2.upgrade() {
                do_start_sftp_session(&ui, &app_state2, &profile2, password, None);
            }
        });
    } else if key_has_passphrase {
        let profile2 = profile.clone();
        prompt_password_then(
            &format!("Key passphrase for {}", profile.name),
            move |key_passphrase| {
                if let Some(ui) = ui_handle.upgrade() {
                    do_start_sftp_session(&ui, &app_state, &profile2, None, key_passphrase);
                }
            },
        );
    } else {
        do_start_sftp_session(ui, &app_state, &profile, None, None);
    }
}

fn do_start_sftp_session(
    ui: &MainWindow,
    app_state: &Rc<RefCell<AppUiState>>,
    profile: &ConnectionProfile,
    password: Option<Zeroizing<String>>,
    key_passphrase: Option<Zeroizing<String>>,
) {
    let (event_tx, event_rx) = async_channel::bounded::<SftpEvent>(256);
    let sftp_cmd_tx =
        crate::ssh::sftp::spawn_sftp_session(profile.clone(), password, key_passphrase, event_tx);

    let session_id = Uuid::new_v4();
    let home = dirs_home();

    let session = SessionHandle {
        id: session_id,
        title: format!("SFTP - {}", profile.name),
        is_terminal: false,
        cmd_tx: None,
        renderer: None,
        font_size: 14.0,
        sftp_cmd_tx: Some(sftp_cmd_tx),
        remote_path: RefCell::new(".".to_string()),
        remote_entries: RefCell::new(Vec::new()),
        local_path: RefCell::new(home.clone()),
    };

    {
        let mut st = app_state.borrow_mut();
        st.sessions.push(session);
        st.active_tab = st.sessions.len() - 1;
    }

    refresh_tabs(ui, app_state);
    update_active_tab_content(ui, app_state);

    // Set initial local files
    let local_items = sftp::read_local_dir(&home);
    ui.set_sftp_local_path(home.display().to_string().into());
    ui.set_sftp_local_files(ModelRc::new(VecModel::from(local_items)));
    ui.set_sftp_status("Connecting...".into());
    ui.set_sftp_transfer_status("Ready for transfers".into());
    ui.set_sftp_connected(false);
    ui.set_sftp_local_selected(-1);
    ui.set_sftp_remote_selected(-1);
    ui.set_sftp_local_summary("No selection".into());
    ui.set_sftp_remote_summary("No selection".into());

    // Spawn async event loop
    let ui_handle = ui.as_weak();
    let app_state_clone = app_state.clone();
    let tab_index = app_state.borrow().sessions.len() - 1;

    slint::spawn_local(async move {
        while let Ok(event) = event_rx.recv().await {
            let ui_handle = ui_handle.clone();
            let app_state = app_state_clone.clone();

            match event {
                SftpEvent::Connected => {
                    if let Some(ui) = ui_handle.upgrade() {
                        ui.set_sftp_status("Connected".into());
                        ui.set_sftp_connected(true);
                    }
                    // Request initial directory listing
                    let state = app_state.borrow();
                    if let Some(session) = state.sessions.get(tab_index) {
                        if let Some(ref tx) = session.sftp_cmd_tx {
                            let _ = tx.send(SftpCommand::ListDir(".".to_string())).await;
                        }
                    }
                }
                SftpEvent::DirListing { path, entries } => {
                    let state = app_state.borrow();
                    if let Some(session) = state.sessions.get(tab_index) {
                        *session.remote_path.borrow_mut() = path.clone();
                        *session.remote_entries.borrow_mut() = entries.clone();

                        if state.active_tab == tab_index {
                            if let Some(ui) = ui_handle.upgrade() {
                                ui.set_sftp_remote_selected(-1);
                                refresh_remote_sftp_view(&ui, session);
                            }
                        }
                    }
                }
                SftpEvent::TransferProgress { name, bytes, total } => {
                    if let Some(ui) = ui_handle.upgrade() {
                        let state = app_state.borrow();
                        if state.active_tab == tab_index {
                            let pct = if total > 0 {
                                (bytes as f64 / total as f64 * 100.0) as u64
                            } else {
                                0
                            };
                            ui.set_sftp_transfer_status(format!("{}: {}%", name, pct).into());
                        }
                    }
                }
                SftpEvent::TransferComplete { name } => {
                    if let Some(ui) = ui_handle.upgrade() {
                        let state = app_state.borrow();
                        if state.active_tab == tab_index {
                            ui.set_sftp_transfer_status(format!("{}: Complete", name).into());
                            // Refresh both panes
                            if let Some(session) = state.sessions.get(tab_index) {
                                ui.set_sftp_local_selected(-1);
                                ui.set_sftp_remote_selected(-1);
                                refresh_local_sftp_view(&ui, session);
                                refresh_remote_sftp_view(&ui, session);

                                if let Some(ref tx) = session.sftp_cmd_tx {
                                    let remote_path = session.remote_path.borrow().clone();
                                    let _ = tx.send(SftpCommand::ListDir(remote_path)).await;
                                }
                            }
                        }
                    }
                }
                SftpEvent::TransferConflict {
                    path,
                    direction,
                    is_dir: _,
                    response_tx,
                } => {
                    if let Some(_ui) = ui_handle.upgrade() {
                        let dir_str = match direction {
                            crate::ssh::sftp::SftpConflictDirection::Upload => "Upload",
                            crate::ssh::sftp::SftpConflictDirection::Download => "Download",
                        };

                        let conflict_dlg = ConflictDialog::new().unwrap();
                        conflict_dlg.set_conflict_path(path.clone().into());
                        conflict_dlg.set_conflict_direction(dir_str.into());
                        conflict_dlg.set_apply_to_all(false);

                        let dlg_weak = conflict_dlg.as_weak();
                        let tx = response_tx.clone();
                        conflict_dlg.on_keep_existing(move || {
                            let dlg = dlg_weak.upgrade().unwrap();
                            let apply_all = dlg.get_apply_to_all();
                            let _ = tx.send_blocking(SftpConflictResponse {
                                decision: crate::ssh::sftp::SftpConflictDecision::KeepExisting,
                                apply_to_all: apply_all,
                            });
                            dlg.hide().ok();
                        });

                        let dlg_weak = conflict_dlg.as_weak();
                        let tx = response_tx.clone();
                        conflict_dlg.on_replace_incoming(move || {
                            let dlg = dlg_weak.upgrade().unwrap();
                            let apply_all = dlg.get_apply_to_all();
                            let _ = tx.send_blocking(SftpConflictResponse {
                                decision:
                                    crate::ssh::sftp::SftpConflictDecision::ReplaceWithIncoming,
                                apply_to_all: apply_all,
                            });
                            dlg.hide().ok();
                        });

                        conflict_dlg.show().ok();
                    }
                }
                SftpEvent::Error(msg) => {
                    log::error!("SFTP error: {}", msg);
                    if let Some(ui) = ui_handle.upgrade() {
                        let state = app_state.borrow();
                        if state.active_tab == tab_index {
                            ui.set_sftp_status(format!("Error: {}", msg).into());
                        }
                    }
                }
                SftpEvent::Disconnected => {
                    if let Some(ui) = ui_handle.upgrade() {
                        let state = app_state.borrow();
                        if state.active_tab == tab_index {
                            ui.set_sftp_status("Disconnected".into());
                            ui.set_sftp_connected(false);
                        }
                    }
                    break;
                }
            }
        }
    })
    .unwrap();
}

fn show_connection_dialog(
    ui: &MainWindow,
    state: &SharedState,
    app_state: &Rc<RefCell<AppUiState>>,
    existing: Option<ConnectionProfile>,
) {
    // Build key names
    let (key_names, key_ids) = dialogs::build_key_names(state);

    let conn_dialog = ConnectionDialog::new().unwrap();

    conn_dialog.set_key_names(ModelRc::new(VecModel::from(key_names)));

    // Track key index to set right before show() (ComboBox resets on model change)
    let mut initial_key_index: i32 = 0;

    if let Some(ref profile) = existing {
        conn_dialog.set_is_edit(true);
        conn_dialog.set_conn_name(profile.name.clone().into());
        conn_dialog.set_hostname(profile.hostname.clone().into());
        conn_dialog.set_port(profile.port as i32);
        conn_dialog.set_username(profile.username.clone().into());
        conn_dialog.set_use_cloudflare_tunnel(profile.use_cloudflare_tunnel);
        conn_dialog.set_auth_method_index(match profile.auth_method {
            AuthMethod::Password => 0,
            AuthMethod::PublicKey => 1,
            AuthMethod::Both => 2,
        });
        if let Some(kid) = profile.key_pair_id {
            if let Some(pos) = key_ids.iter().position(|id| *id == kid) {
                initial_key_index = pos as i32;
            }
        }

        // Populate tunnels
        let tunnel_items: Vec<TunnelItem> = profile
            .tunnels
            .iter()
            .map(|tc| TunnelItem {
                id: tc.id.to_string().into(),
                name: tc.name.clone().into(),
                description: format!(
                    "{}:{} -> {}:{}",
                    tc.local_host, tc.local_port, tc.remote_host, tc.remote_port
                )
                .into(),
                enabled: tc.enabled,
            })
            .collect();
        conn_dialog.set_tunnels(ModelRc::new(VecModel::from(tunnel_items)));

        let mut ast = app_state.borrow_mut();
        ast.editing_profile_id = Some(profile.id);
        ast.editing_profile_created_at = Some(profile.created_at);
        ast.pending_tunnels = profile.tunnels.clone();
    } else {
        let mut ast = app_state.borrow_mut();
        ast.editing_profile_id = None;
        ast.editing_profile_created_at = None;
        ast.pending_tunnels = Vec::new();
    }

    // Save callback
    {
        let dialog_handle = conn_dialog.as_weak();
        let state = state.clone();
        let ui_handle = ui.as_weak();
        let app_state = app_state.clone();
        let key_ids = key_ids.clone();
        conn_dialog.on_save_clicked(move || {
            if let Some(dialog) = dialog_handle.upgrade() {
                let ast = app_state.borrow();
                let tunnels = ast.pending_tunnels.clone();
                let existing_id = ast.editing_profile_id;
                let existing_created = ast.editing_profile_created_at;
                drop(ast);

                if let Err(e) = dialogs::save_connection_profile(
                    &state,
                    dialog.get_conn_name().as_str(),
                    dialog.get_hostname().as_str(),
                    dialog.get_port() as u16,
                    dialog.get_username().as_str(),
                    dialog.get_use_cloudflare_tunnel(),
                    dialog.get_auth_method_index(),
                    &key_ids,
                    dialog.get_key_index(),
                    tunnels,
                    existing_id,
                    existing_created,
                ) {
                    log::error!("Failed to save connection: {e}");
                    return;
                }
                dialog.hide().unwrap();
                if let Some(ui) = ui_handle.upgrade() {
                    refresh_connection_list(&ui, &state);
                }
            }
        });
    }

    {
        let dialog_handle = conn_dialog.as_weak();
        conn_dialog.on_cancel_clicked(move || {
            if let Some(dialog) = dialog_handle.upgrade() {
                dialog.hide().unwrap();
            }
        });
    }

    // Add tunnel
    {
        let app_state = app_state.clone();
        let dialog_handle = conn_dialog.as_weak();
        conn_dialog.on_add_tunnel_clicked(move || {
            let tunnel_dialog = TunnelDialog::new().unwrap();

            let td_handle = tunnel_dialog.as_weak();
            let app_state_c = app_state.clone();
            let dialog_handle_c = dialog_handle.clone();
            tunnel_dialog.on_save_clicked(move || {
                if let Some(td) = td_handle.upgrade() {
                    if let Some(tc) = dialogs::create_tunnel_config(
                        td.get_tunnel_name().as_str(),
                        td.get_local_host().as_str(),
                        td.get_local_port() as u16,
                        td.get_remote_host().as_str(),
                        td.get_remote_port() as u16,
                        td.get_tunnel_enabled(),
                    ) {
                        let mut ast = app_state_c.borrow_mut();
                        ast.pending_tunnels.push(tc.clone());

                        // Update tunnel list in connection dialog
                        if let Some(cd) = dialog_handle_c.upgrade() {
                            let items: Vec<TunnelItem> = ast
                                .pending_tunnels
                                .iter()
                                .map(|t| TunnelItem {
                                    id: t.id.to_string().into(),
                                    name: t.name.clone().into(),
                                    description: format!(
                                        "{}:{} -> {}:{}",
                                        t.local_host, t.local_port, t.remote_host, t.remote_port
                                    )
                                    .into(),
                                    enabled: t.enabled,
                                })
                                .collect();
                            cd.set_tunnels(ModelRc::new(VecModel::from(items)));
                        }

                        td.hide().unwrap();
                    }
                }
            });

            let td_handle2 = tunnel_dialog.as_weak();
            tunnel_dialog.on_cancel_clicked(move || {
                if let Some(td) = td_handle2.upgrade() {
                    td.hide().unwrap();
                }
            });

            tunnel_dialog.show().unwrap();
        });
    }

    // Remove tunnel callback
    {
        let app_state = app_state.clone();
        let dialog_handle = conn_dialog.as_weak();
        conn_dialog.on_remove_tunnel_clicked(move |idx| {
            let mut ast = app_state.borrow_mut();
            let idx = idx as usize;
            if idx < ast.pending_tunnels.len() {
                ast.pending_tunnels.remove(idx);
                let items: Vec<TunnelItem> = ast
                    .pending_tunnels
                    .iter()
                    .map(|t| TunnelItem {
                        id: t.id.to_string().into(),
                        name: t.name.clone().into(),
                        description: format!(
                            "{}:{} -> {}:{}",
                            t.local_host, t.local_port, t.remote_host, t.remote_port
                        )
                        .into(),
                        enabled: t.enabled,
                    })
                    .collect();
                if let Some(cd) = dialog_handle.upgrade() {
                    cd.set_tunnels(ModelRc::new(VecModel::from(items)));
                }
            }
        });
    }

    // Set key index and show dialog
    conn_dialog.set_key_index(initial_key_index);
    conn_dialog.show().unwrap();
    // Force ComboBox to apply the key index after show via timer,
    // because the ComboBox resets current-index when the dialog renders.
    {
        let dialog_handle = conn_dialog.as_weak();
        let ki = initial_key_index;
        slint::Timer::single_shot(std::time::Duration::from_millis(0), move || {
            if let Some(d) = dialog_handle.upgrade() {
                d.set_key_index(ki);
                d.invoke_apply_key_index();
            }
        });
    }
}

fn show_import_key_dialog(
    state: &SharedState,
    on_imported: impl Fn() + 'static,
) {
    let dialog = ImportKeyDialog::new().unwrap();

    {
        let dialog_handle = dialog.as_weak();
        dialog.on_browse_private_clicked(move || {
            let file = rfd::FileDialog::new()
                .set_title("Select Private Key File")
                .pick_file();
            if let Some(path) = file {
                if let Some(d) = dialog_handle.upgrade() {
                    d.set_import_private_path(path.display().to_string().into());
                }
            }
        });
    }

    {
        let dialog_handle = dialog.as_weak();
        dialog.on_browse_public_clicked(move || {
            let file = rfd::FileDialog::new()
                .set_title("Select Public Key File")
                .pick_file();
            if let Some(path) = file {
                if let Some(d) = dialog_handle.upgrade() {
                    d.set_import_public_path(path.display().to_string().into());
                }
            }
        });
    }

    {
        let state = state.clone();
        let dialog_handle = dialog.as_weak();
        dialog.on_import_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                match dialogs::import_key(
                    &state,
                    d.get_import_name().as_str(),
                    d.get_import_private_path().as_str(),
                    d.get_import_public_path().as_str(),
                ) {
                    Ok(msg) => {
                        log::info!("{}", msg);
                        on_imported();
                        d.hide().ok();
                    }
                    Err(e) => log::error!("{}", e),
                }
            }
        });
    }

    {
        let dialog_handle = dialog.as_weak();
        dialog.on_cancel_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                d.hide().ok();
            }
        });
    }

    dialog.show().ok();
}

fn show_key_manager(_ui: &MainWindow, state: &SharedState) {
    let dialog = KeyManagerDialog::new().unwrap();

    // Populate stored keys
    let key_items = dialogs::build_key_items(state);
    dialog.set_stored_keys(ModelRc::new(VecModel::from(key_items)));

    // Generate
    {
        let state = state.clone();
        let dialog_handle = dialog.as_weak();
        dialog.on_generate_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                match dialogs::generate_key(
                    &state,
                    d.get_gen_name().as_str(),
                    d.get_gen_passphrase().as_str(),
                    d.get_gen_algo_index(),
                ) {
                    Ok(_) => {
                        d.set_gen_name("".into());
                        d.set_gen_passphrase("".into());
                        let items = dialogs::build_key_items(&state);
                        d.set_stored_keys(ModelRc::new(VecModel::from(items)));
                    }
                    Err(e) => log::error!("{}", e),
                }
            }
        });
    }

    // Open import dialog
    {
        let state = state.clone();
        let dialog_handle = dialog.as_weak();
        dialog.on_open_import_clicked(move || {
            let state = state.clone();
            let refresh_state = state.clone();
            let dialog_handle = dialog_handle.clone();
            show_import_key_dialog(&state, move || {
                if let Some(d) = dialog_handle.upgrade() {
                    let items = dialogs::build_key_items(&refresh_state);
                    d.set_stored_keys(ModelRc::new(VecModel::from(items)));
                }
            });
        });
    }

    // Copy public key
    {
        let state = state.clone();
        dialog.on_copy_public_key(move |idx| {
            match dialogs::copy_public_key(&state, idx as usize) {
                Ok(pub_key) => {
                    match arboard::Clipboard::new() {
                        Ok(mut clipboard) => {
                            if let Err(e) = clipboard.set_text(pub_key) {
                                log::error!("Failed to copy to clipboard: {e}");
                            }
                        }
                        Err(e) => log::error!("Failed to open clipboard: {e}"),
                    }
                }
                Err(e) => log::error!("{}", e),
            }
        });
    }

    // Delete key
    {
        let state = state.clone();
        let dialog_handle = dialog.as_weak();
        dialog.on_delete_key(move |idx| {
            dialogs::delete_key_by_index(&state, idx as usize);
            if let Some(d) = dialog_handle.upgrade() {
                let items = dialogs::build_key_items(&state);
                d.set_stored_keys(ModelRc::new(VecModel::from(items)));
            }
        });
    }

    // Backup keys (encrypted)
    {
        let state = state.clone();
        dialog.on_backup_clicked(move || {
            let state = state.clone();
            prompt_password_then("Set a passphrase to encrypt the backup", move |passphrase| {
                let passphrase = match passphrase {
                    Some(p) if !p.is_empty() => p,
                    _ => {
                        log::warn!("Key backup cancelled — passphrase required");
                        return;
                    }
                };
                let store = state.key_store.lock().unwrap();
                match store.export_encrypted_backup(&passphrase) {
                    Ok(data) => {
                        let file = rfd::FileDialog::new()
                            .set_title("Save Encrypted Key Backup")
                            .set_file_name("wrustyssh-keys-backup.enc")
                            .add_filter("Encrypted Backup", &["enc"])
                            .save_file();
                        if let Some(path) = file {
                            if let Err(e) = std::fs::write(&path, &data) {
                                log::error!("Failed to write backup: {e}");
                            }
                        }
                    }
                    Err(e) => log::error!("Failed to export keys: {e}"),
                }
            });
        });
    }

    // Restore keys (encrypted)
    {
        let state = state.clone();
        let dialog_handle = dialog.as_weak();
        dialog.on_restore_clicked(move || {
            let file = rfd::FileDialog::new()
                .set_title("Restore Keys from Encrypted Backup")
                .add_filter("Encrypted Backup", &["enc"])
                .add_filter("Legacy JSON", &["json"])
                .pick_file();
            if let Some(path) = file {
                let state = state.clone();
                let dialog_handle = dialog_handle.clone();
                let is_legacy = path.extension().map(|e| e == "json").unwrap_or(false);

                if is_legacy {
                    // Support importing old unencrypted backups
                    match std::fs::read_to_string(&path) {
                        Ok(json) => {
                            let mut store = state.key_store.lock().unwrap();
                            match store.import_backup(&json) {
                                Ok(count) => {
                                    log::info!("Imported {count} key(s) from legacy backup.");
                                    drop(store);
                                    if let Some(d) = dialog_handle.upgrade() {
                                        let items = dialogs::build_key_items(&state);
                                        d.set_stored_keys(ModelRc::new(VecModel::from(items)));
                                    }
                                }
                                Err(e) => log::error!("Failed to import backup: {e}"),
                            }
                        }
                        Err(e) => log::error!("Failed to read backup file: {e}"),
                    }
                } else {
                    prompt_password_then(
                        "Enter the backup passphrase",
                        move |passphrase| {
                            let passphrase = match passphrase {
                                Some(p) if !p.is_empty() => p,
                                _ => {
                                    log::warn!("Key restore cancelled — passphrase required");
                                    return;
                                }
                            };
                            match std::fs::read(&path) {
                                Ok(data) => {
                                    let mut store = state.key_store.lock().unwrap();
                                    match store.import_encrypted_backup(&data, &passphrase) {
                                        Ok(count) => {
                                            log::info!("Imported {count} key(s).");
                                            drop(store);
                                            if let Some(d) = dialog_handle.upgrade() {
                                                let items = dialogs::build_key_items(&state);
                                                d.set_stored_keys(ModelRc::new(VecModel::from(
                                                    items,
                                                )));
                                            }
                                        }
                                        Err(e) => log::error!("Failed to import backup: {e}"),
                                    }
                                }
                                Err(e) => log::error!("Failed to read backup file: {e}"),
                            }
                        },
                    );
                }
            }
        });
    }

    // Close
    {
        let dialog_handle = dialog.as_weak();
        dialog.on_close_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                d.hide().unwrap();
            }
        });
    }

    dialog.show().unwrap();
}

fn show_preferences(ui: &MainWindow, state: &SharedState) {
    let dialog = PreferencesDialog::new().unwrap();
    let fonts = available_font_families();
    dialog.set_font_options(ModelRc::new(VecModel::from(fonts.clone())));
    let palette_names: Vec<slint::SharedString> = crate::ui::terminal::terminal_palette_names()
        .into_iter()
        .map(Into::into)
        .collect();
    dialog.set_terminal_palettes(ModelRc::new(VecModel::from(palette_names)));

    let current = state.settings.lock().unwrap().clone();
    dialog.set_font_family(current.font_family.clone().into());
    dialog.set_font_size(current.font_size as i32);
    dialog.set_scrollback_lines(current.scrollback_lines as i32);
    dialog.set_terminal_type(current.default_terminal_type.clone().into());
    dialog.set_app_font_family(current.app_font_family.clone().into());
    dialog.set_app_font_size(current.app_font_size as i32);
    dialog.set_button_font_size(current.button_font_size as i32);
    dialog.set_connection_name_font_size(current.connection_name_font_size as i32);
    dialog.set_connection_subtitle_font_size(current.connection_subtitle_font_size as i32);
    dialog.set_font_family_index(font_index(&fonts, &current.font_family));
    dialog.set_app_font_family_index(font_index(&fonts, &current.app_font_family));
    dialog.set_terminal_palette_index(crate::ui::terminal::terminal_palette_index(
        &current.terminal_color_scheme,
    ));
    {
        let state = state.clone();
        let dialog_handle = dialog.as_weak();
        let ui_handle = ui.as_weak();
        dialog.on_save_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                // Read font family names directly from the string properties
                // (updated by ComboBox selected callbacks), not from index lookups
                let font_family = d.get_font_family().to_string();
                let app_font_family = d.get_app_font_family().to_string();
                let _ = dialogs::save_preferences(
                    &state,
                    &font_family,
                    d.get_font_size() as u32,
                    d.get_scrollback_lines() as i64,
                    d.get_terminal_type().as_str(),
                    crate::ui::terminal::terminal_palette_name_by_index(
                        d.get_terminal_palette_index(),
                    ),
                    &app_font_family,
                    d.get_app_font_size() as u32,
                    d.get_button_font_size() as u32,
                    d.get_connection_name_font_size() as u32,
                    d.get_connection_subtitle_font_size() as u32,
                );
                d.hide().unwrap();
            }
            // Apply saved settings immediately
            if let Some(ui) = ui_handle.upgrade() {
                let saved = state.settings.lock().unwrap().clone();
                apply_ui_typography(&ui, &saved);
            }
        });
    }

    {
        let dialog_handle = dialog.as_weak();
        let fonts = fonts.clone();
        dialog.on_restore_defaults_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                let defaults = Settings::default();
                d.set_font_family(defaults.font_family.clone().into());
                d.set_font_size(defaults.font_size as i32);
                d.set_scrollback_lines(defaults.scrollback_lines as i32);
                d.set_terminal_type(defaults.default_terminal_type.into());
                d.set_app_font_family(defaults.app_font_family.clone().into());
                d.set_app_font_size(defaults.app_font_size as i32);
                d.set_button_font_size(defaults.button_font_size as i32);
                d.set_connection_name_font_size(defaults.connection_name_font_size as i32);
                d.set_connection_subtitle_font_size(defaults.connection_subtitle_font_size as i32);
                d.set_font_family_index(font_index(&fonts, &defaults.font_family));
                d.set_app_font_family_index(font_index(&fonts, &defaults.app_font_family));
                d.set_terminal_palette_index(crate::ui::terminal::terminal_palette_index(
                    &defaults.terminal_color_scheme,
                ));
            }
        });
    }

    {
        let dialog_handle = dialog.as_weak();
        dialog.on_cancel_clicked(move || {
            if let Some(d) = dialog_handle.upgrade() {
                d.hide().unwrap();
            }
        });
    }

    // Set indexes right before show — ComboBox model binding may reset them
    let app_idx = font_index(&fonts, &current.app_font_family);
    let term_idx = font_index(&fonts, &current.font_family);
    dialog.set_font_family_index(term_idx);
    dialog.set_app_font_family_index(app_idx);
    dialog.show().unwrap();
    // Re-set after show in case the ComboBox reset during initialization
    dialog.set_font_family_index(term_idx);
    dialog.set_app_font_family_index(app_idx);
}
