use gtk4 as gtk;
use gtk::prelude::*;
use gtk::glib;
use libadwaita as adw;
use adw::prelude::*;

use crate::app::SharedState;
use crate::ui::connection_list;
use crate::ui::key_manager_dialog;
use crate::ui::preferences_dialog;
use crate::ui::terminal_tab;

pub fn build_window(app: &adw::Application, state: SharedState) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("GrustySSH")
        .default_width(1200)
        .default_height(800)
        .build();

    // Load CSS
    load_css();

    // Main layout: NavigationSplitView
    let split_view = adw::NavigationSplitView::new();

    // Content side: tab bar + tab view
    let tab_view = adw::TabView::new();
    let tab_bar = adw::TabBar::builder()
        .view(&tab_view)
        .autohide(false)
        .build();

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // Header bar with menu
    let header_bar = adw::HeaderBar::new();

    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Menu")
        .build();

    let menu = gtk::gio::Menu::new();
    menu.append(Some("SSH Key Manager"), Some("app.key-manager"));
    menu.append(Some("Preferences"), Some("app.preferences"));
    menu.append(Some("About"), Some("app.about"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    menu_btn.set_popover(Some(&popover));
    header_bar.pack_end(&menu_btn);

    content_box.append(&header_bar);
    content_box.append(&tab_bar);
    content_box.append(&tab_view);

    // Sidebar: connection list
    let (sidebar, _rebuild_list) = connection_list::build_connection_list(
        &window,
        &tab_view,
        &state,
    );

    let sidebar_page = adw::NavigationPage::builder()
        .title("Connections")
        .child(&sidebar)
        .build();

    let content_page = adw::NavigationPage::builder()
        .title("Grustyssh")
        .child(&content_box)
        .build();

    split_view.set_sidebar(Some(&sidebar_page));
    split_view.set_content(Some(&content_page));

    window.set_content(Some(&split_view));

    // Auto-focus terminal when switching tabs.
    // We need an idle callback because the page may not be fully mapped yet
    // when the selected-page signal fires.
    tab_view.connect_notify(Some("selected-page"), |tab_view, _| {
        if let Some(page) = tab_view.selected_page() {
            let child = page.child();
            glib::idle_add_local_once(move || {
                // Walk into the ScrolledWindow to find the VTE terminal
                fn find_terminal(widget: &gtk::Widget) -> Option<vte4::Terminal> {
                    if let Some(term) = widget.clone().downcast::<vte4::Terminal>().ok() {
                        return Some(term);
                    }
                    // Check first child, then siblings
                    if let Some(child) = widget.first_child() {
                        let mut current = Some(child);
                        while let Some(c) = current {
                            if let Some(term) = find_terminal(&c) {
                                return Some(term);
                            }
                            current = c.next_sibling();
                        }
                    }
                    None
                }
                if let Some(terminal) = find_terminal(&child) {
                    terminal.grab_focus();
                }
            });
        }
    });

    // Tab close handler: disconnect SSH session
    tab_view.connect_close_page(|tab_view, page| {
        terminal_tab::disconnect_tab(page);
        tab_view.close_page_finish(page, true);
        glib::Propagation::Stop
    });

    // Window close handler: disconnect all sessions
    let tab_view_close = tab_view.clone();
    window.connect_close_request(move |_| {
        let n = tab_view_close.n_pages();
        for i in 0..n {
            let page = tab_view_close.nth_page(i);
            terminal_tab::disconnect_tab(&page);
        }
        glib::Propagation::Proceed
    });

    // App actions
    let window_for_keys = window.clone();
    let state_for_keys = state.clone();
    let key_manager_action = gtk::gio::SimpleAction::new("key-manager", None);
    key_manager_action.connect_activate(move |_, _| {
        key_manager_dialog::show_key_manager_dialog(&window_for_keys, &state_for_keys);
    });
    app.add_action(&key_manager_action);

    let window_for_prefs = window.clone();
    let state_for_prefs = state.clone();
    let preferences_action = gtk::gio::SimpleAction::new("preferences", None);
    preferences_action.connect_activate(move |_, _| {
        preferences_dialog::show_preferences_dialog(&window_for_prefs, &state_for_prefs);
    });
    app.add_action(&preferences_action);

    let about_action = gtk::gio::SimpleAction::new("about", None);
    let window_for_about = window.clone();
    about_action.connect_activate(move |_, _| {
        let about = adw::AboutDialog::builder()
            .application_name("GrustySSH")
            .application_icon("grustyssh")
            .version("1.9.0")
            .developer_name("GrustySSH Project")
            .comments("A GTK4/libadwaita SSH client with tabbed terminals")
            .build();
        about.present(Some(&window_for_about));
    });
    app.add_action(&about_action);

    window
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    let css = include_str!("style.css");
    provider.load_from_string(css);

    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("No display found"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
