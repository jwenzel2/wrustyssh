use super::window::ConnectionItem;
use crate::app::SharedState;
use crate::models::connection::ConnectionProfile;

/// Build the connection item list from the profile store.
pub fn build_connection_items(state: &SharedState, filter: &str) -> Vec<ConnectionItem> {
    let store = state.profile_store.lock().unwrap();
    let filter = filter.trim().to_ascii_lowercase();
    store
        .profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            if filter.is_empty() {
                return true;
            }

            let name = p.name.to_ascii_lowercase();
            let subtitle = format!("{}@{}:{}", p.username, p.hostname, p.port).to_ascii_lowercase();
            name.contains(&filter) || subtitle.contains(&filter)
        })
        .map(|(idx, p)| ConnectionItem {
            id: p.id.to_string().into(),
            name: p.name.clone().into(),
            subtitle: format!("{}@{}:{}", p.username, p.hostname, p.port).into(),
            index: idx as i32,
        })
        .collect()
}

/// Get a profile by index.
pub fn get_profile_by_index(state: &SharedState, index: usize) -> Option<ConnectionProfile> {
    let store = state.profile_store.lock().unwrap();
    store.profiles.get(index).cloned()
}
