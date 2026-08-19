//! Styled transcript cache.
//!
//! Finished messages are immutable. Re-parsing their markdown and running
//! syntect for every streaming redraw makes a session progressively slower, so
//! this cache pays that work once per message and invalidates only when a fact
//! that changes rendering changes.

use std::collections::{BTreeMap, HashMap, HashSet};

use ratatui::text::Line;
use uuid::Uuid;

use crate::tui::image::Placement;
use crate::tui::theme::ThemeId;

#[derive(Debug)]
struct CachedMessage {
    generation: u64,
    lines: Vec<Line<'static>>,
    /// Image placements, offset from the first line of *this message*. The
    /// absolute row is only knowable once the whole transcript is assembled,
    /// so the cache stores the part that does not move.
    images: Vec<Placement>,
}

#[derive(Debug)]
pub(crate) struct RenderCache {
    entries: BTreeMap<Uuid, CachedMessage>,
    call_index: HashMap<String, Uuid>,
    seen_results: HashSet<String>,
    generation: u64,
    width: u16,
    theme: ThemeId,
    show_details: bool,
    #[cfg(test)]
    builds: usize,
}

impl Default for RenderCache {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            call_index: HashMap::new(),
            seen_results: HashSet::new(),
            generation: 0,
            width: 0,
            theme: ThemeId::default(),
            show_details: true,
            #[cfg(test)]
            builds: 0,
        }
    }
}

impl RenderCache {
    pub(crate) fn prepare(&mut self, width: u16, theme: ThemeId, show_details: bool) {
        if self.width != width || self.theme != theme || self.show_details != show_details {
            self.generation = self.generation.wrapping_add(1);
            self.entries.clear();
            self.call_index.clear();
            self.seen_results.clear();
            self.width = width;
            self.theme = theme;
            self.show_details = show_details;
        }
    }

    pub(crate) fn register_call(&mut self, call_id: &str, message_id: Uuid) {
        self.call_index.insert(call_id.to_owned(), message_id);
    }

    pub(crate) fn note_result(&mut self, call_id: &str) {
        if self.seen_results.insert(call_id.to_owned())
            && let Some(owner) = self.call_index.get(call_id)
        {
            self.entries.remove(owner);
        }
    }

    pub(crate) fn get(&self, id: Uuid) -> Option<(Vec<Line<'static>>, Vec<Placement>)> {
        self.entries
            .get(&id)
            .filter(|entry| entry.generation == self.generation)
            .map(|entry| (entry.lines.clone(), entry.images.clone()))
    }

    pub(crate) fn insert(&mut self, id: Uuid, lines: Vec<Line<'static>>, images: Vec<Placement>) {
        #[cfg(test)]
        {
            self.builds = self.builds.saturating_add(1);
        }
        self.entries.insert(
            id,
            CachedMessage {
                generation: self.generation,
                lines,
                images,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_lines_survive_without_a_generation_change() {
        let mut cache = RenderCache::default();
        let id = Uuid::now_v7();
        cache.prepare(80, ThemeId::Noir, false);
        cache.insert(id, vec![Line::from("message")], Vec::new());
        assert!(cache.get(id).is_some());
        assert_eq!(cache.builds, 1);
        cache.prepare(80, ThemeId::Noir, false);
        assert!(cache.get(id).is_some());
        assert_eq!(cache.builds, 1);
    }

    #[test]
    fn result_arrival_invalidates_only_its_owner() {
        let mut cache = RenderCache::default();
        let owner = Uuid::now_v7();
        let other = Uuid::now_v7();
        cache.prepare(80, ThemeId::Noir, false);
        cache.register_call("call-1", owner);
        cache.insert(owner, vec![Line::from("running")], Vec::new());
        cache.insert(other, vec![Line::from("other")], Vec::new());
        cache.note_result("call-1");
        assert!(cache.get(owner).is_none());
        assert!(cache.get(other).is_some());
    }

    #[test]
    fn theme_change_invalidates_every_message() {
        let mut cache = RenderCache::default();
        let id = Uuid::now_v7();
        cache.prepare(80, ThemeId::Noir, false);
        cache.insert(id, vec![Line::from("message")], Vec::new());
        cache.prepare(80, ThemeId::Mono, false);
        assert!(cache.get(id).is_none());
    }
}
