use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use adw::prelude::*;

use std::cell::RefCell;
use std::rc::Rc;
use uuid::Uuid;

use crate::app::SharedState;
use crate::models::connection::{AuthMethod, ConnectionProfile};
use crate::models::tunnel::TunnelConfig;

/// Show a dialog to create or edit a connection profile.
/// `existing` is Some for editing, None for creating new.
/// `on_save` is called with the saved profile.
pub fn show_connection_dialog(
    parent: &adw::ApplicationWindow,
    state: &SharedState,
    existing: Option<ConnectionProfile>,
    on_save: impl Fn(ConnectionProfile) + 'static,
) {
    let is_edit = existing.is_some();
    let dialog = adw::Dialog::builder()
        .title(if is_edit {
            "Edit Connection"
        } else {
            "New Connection"
        })
        .content_width(500)
        .content_height(600)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let save_btn = gtk::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&save_btn);

    toolbar_view.add_top_bar(&header);

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content_box.set_margin_start(16);
    content_box.set_margin_end(16);
    content_box.set_margin_top(8);
    content_box.set_margin_bottom(16);

    // Connection details group
    let details_group = adw::PreferencesGroup::builder()
        .title("Connection Details")
        .build();

    let name_row = adw::EntryRow::builder().title("Name").build();
    let host_row = adw::EntryRow::builder().title("Hostname").build();

    let port_adjustment =
        gtk::Adjustment::new(22.0, 1.0, 65535.0, 1.0, 10.0, 0.0);
    let port_row = adw::SpinRow::builder()
        .title("Port")
        .adjustment(&port_adjustment)
        .build();

    let user_row = adw::EntryRow::builder().title("Username").build();

    details_group.add(&name_row);
    details_group.add(&host_row);
    details_group.add(&port_row);
    details_group.add(&user_row);
    content_box.append(&details_group);

    // Authentication group
    let auth_group = adw::PreferencesGroup::builder()
        .title("Authentication")
        .build();

    let auth_method_row = adw::ComboRow::builder()
        .title("Method")
        .build();
    let auth_list = gtk::StringList::new(&["Password", "Public Key", "Both"]);
    auth_method_row.set_model(Some(&auth_list));

    let key_row = adw::ComboRow::builder()
        .title("SSH Key")
        .build();

    // Populate keys
    let key_ids: Rc<RefCell<Vec<Uuid>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let store = state.key_store.lock().unwrap();
        let mut names: Vec<String> = Vec::new();
        let mut ids: Vec<Uuid> = Vec::new();
        names.push("(None)".into());
        ids.push(Uuid::nil());
        for k in &store.keys {
            names.push(format!("{} ({})", k.name, k.algorithm));
            ids.push(k.id);
        }
        let str_list = gtk::StringList::new(
            &names.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        );
        key_row.set_model(Some(&str_list));
        *key_ids.borrow_mut() = ids;
    }

    auth_group.add(&auth_method_row);
    auth_group.add(&key_row);
    content_box.append(&auth_group);

    // Grey out key row when Password is selected, since it's not needed.
    // Grey out nothing when Public Key or Both is selected (key is needed).
    let key_row_for_auth = key_row.clone();
    let update_auth_sensitivity = move |selected: u32| {
        match selected {
            0 => {
                // Password only: disable key selection
                key_row_for_auth.set_sensitive(false);
            }
            1 | 2 => {
                // Public Key or Both: enable key selection
                key_row_for_auth.set_sensitive(true);
            }
            _ => {}
        }
    };
    // Set initial state
    update_auth_sensitivity(auth_method_row.selected());

    let update_fn = update_auth_sensitivity.clone();
    auth_method_row.connect_selected_notify(move |row| {
        update_fn(row.selected());
    });

    // Tunnels group
    let tunnels_group = adw::PreferencesGroup::builder()
        .title("Tunnels")
        .description("Local port forwarding")
        .build();

    let tunnels: Rc<RefCell<Vec<TunnelConfig>>> = Rc::new(RefCell::new(Vec::new()));

    let tunnels_listbox = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();

    let add_tunnel_btn = gtk::Button::builder()
        .label("Add Tunnel")
        .css_classes(["flat"])
        .halign(gtk::Align::Start)
        .margin_top(4)
        .build();

    content_box.append(&tunnels_group);
    content_box.append(&tunnels_listbox);
    content_box.append(&add_tunnel_btn);

    // Populate existing values if editing
    let profile_id;
    let created_at;
    if let Some(ref profile) = existing {
        name_row.set_text(&profile.name);
        host_row.set_text(&profile.hostname);
        port_row.set_value(profile.port as f64);
        user_row.set_text(&profile.username);

        let auth_idx = match profile.auth_method {
            AuthMethod::Password => 0,
            AuthMethod::PublicKey => 1,
            AuthMethod::Both => 2,
        };
        auth_method_row.set_selected(auth_idx);

        if let Some(kid) = profile.key_pair_id {
            let ids = key_ids.borrow();
            if let Some(pos) = ids.iter().position(|id| *id == kid) {
                key_row.set_selected(pos as u32);
            }
        }

        *tunnels.borrow_mut() = profile.tunnels.clone();
        profile_id = profile.id;
        created_at = profile.created_at;

        // Rebuild tunnel list display
        for tc in &profile.tunnels {
            let row = adw::ActionRow::builder()
                .title(&tc.name)
                .subtitle(&format!(
                    "{}:{} → {}:{}",
                    tc.local_host, tc.local_port, tc.remote_host, tc.remote_port
                ))
                .build();
            tunnels_listbox.append(&row);
        }
    } else {
        profile_id = Uuid::new_v4();
        created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
    }

    // Add tunnel button
    let tunnels_clone = tunnels.clone();
    let tunnels_listbox_clone = tunnels_listbox.clone();
    let _dialog_ref = dialog.clone();
    let parent_clone = parent.clone();
    add_tunnel_btn.connect_clicked(move |_| {
        crate::ui::tunnel_dialog::show_tunnel_dialog(
            &parent_clone,
            None,
            {
                let tunnels_c = tunnels_clone.clone();
                let listbox_c = tunnels_listbox_clone.clone();
                move |tc: TunnelConfig| {
                    let row = adw::ActionRow::builder()
                        .title(&tc.name)
                        .subtitle(&format!(
                            "{}:{} → {}:{}",
                            tc.local_host, tc.local_port, tc.remote_host, tc.remote_port
                        ))
                        .build();
                    listbox_c.append(&row);
                    tunnels_c.borrow_mut().push(tc);
                }
            },
        );
    });

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&content_box)
        .vexpand(true)
        .build();
    toolbar_view.set_content(Some(&scrolled));
    dialog.set_child(Some(&toolbar_view));

    // Enter key in entry rows triggers save
    {
        let btn = save_btn.clone();
        name_row.connect_entry_activated(move |_| { btn.emit_clicked(); });
    }
    {
        let btn = save_btn.clone();
        host_row.connect_entry_activated(move |_| { btn.emit_clicked(); });
    }
    {
        let btn = save_btn.clone();
        user_row.connect_entry_activated(move |_| { btn.emit_clicked(); });
    }

    // Save button handler
    let dialog_clone = dialog.clone();
    let key_ids_clone = key_ids.clone();
    let tunnels_clone = tunnels.clone();
    save_btn.connect_clicked(move |_| {
        let name = name_row.text().to_string();
        let hostname = host_row.text().to_string();
        let port = port_row.value() as u16;
        let username = user_row.text().to_string();

        if name.is_empty() || hostname.is_empty() || username.is_empty() {
            return;
        }

        let auth_method = match auth_method_row.selected() {
            0 => AuthMethod::Password,
            1 => AuthMethod::PublicKey,
            2 => AuthMethod::Both,
            _ => AuthMethod::Password,
        };

        let key_idx = key_row.selected() as usize;
        let ids = key_ids_clone.borrow();
        let key_pair_id = if key_idx > 0 && key_idx < ids.len() {
            Some(ids[key_idx])
        } else {
            None
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let profile = ConnectionProfile {
            id: profile_id,
            name,
            hostname,
            port,
            username,
            auth_method,
            key_pair_id,
            tunnels: tunnels_clone.borrow().clone(),
            created_at,
            updated_at: now,
        };

        on_save(profile);
        dialog_clone.close();
    });

    dialog.present(Some(parent));
}
