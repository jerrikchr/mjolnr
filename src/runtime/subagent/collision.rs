//! Read-set collision detection across a settled spawn group.
//!
//! A spawn group is concurrent by construction: siblings run in separate
//! worktrees at the same time, so "same workspace-relative path" is the one
//! thing they share the moment they both touch it. This module names that
//! overlap. It is pure — no worktrees, no store, no events — so the gate it
//! feeds can be tested without a process spawn, and the runtime layers the
//! durable record and the refusal on top of it.

use std::collections::BTreeSet;

use crate::core::event::SessionId;

/// One child per settled agent: the workspace-relative paths it read (its
/// durable `read_evidence`) and the paths its branch changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTouch {
    pub id: SessionId,
    pub read: Vec<String>,
    pub wrote: Vec<String>,
}

/// A stale read: `writer`'s mutation touched `path`, which `reader` had read.
///
/// `reader`'s read of `path` is now stale, so `reader` may not claim a
/// verified finish without re-reading it (AGENTS.md §1–2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub reader: SessionId,
    pub writer: SessionId,
    pub path: String,
}

/// Detect every read-set collision in a settled group.
///
/// A collision is *some agent `writer` mutated a path a **different** agent
/// `reader` had read*. Self-collisions are excluded by construction — an agent
/// that reads then edits its own file performs an ordinary edit, not a
/// collision — and a reader whose path is written after its read is the only
/// party whose finish is invalidated: the writer owns the new content, the
/// reader owns a now-stale observation.
///
/// Deterministic (AGENTS.md §7): the result is ordered by
/// `(reader, writer, path)`, read paths are compared as a set so input list
/// order cannot change the answer, and a path written by two siblings yields
/// two collisions rather than one guessed owner.
#[must_use]
pub fn detect(agents: &[AgentTouch]) -> Vec<Collision> {
    let mut collisions = Vec::new();
    for reader in agents {
        let read: BTreeSet<&str> = reader.read.iter().map(String::as_str).collect();
        for writer in agents.iter().filter(|agent| agent.id != reader.id) {
            for path in &writer.wrote {
                if read.contains(path.as_str()) {
                    collisions.push(Collision {
                        reader: reader.id,
                        writer: writer.id,
                        path: path.clone(),
                    });
                }
            }
        }
    }
    collisions.sort_unstable_by_key(|collision| {
        (collision.reader, collision.writer, collision.path.clone())
    });
    collisions
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    #[test]
    fn a_sibling_write_invalidates_a_readers_path() {
        let reader = SessionId::new();
        let writer = SessionId::new();
        let collisions = detect(&[
            AgentTouch {
                id: reader,
                read: vec!["src/a.rs".to_owned()],
                wrote: Vec::new(),
            },
            AgentTouch {
                id: writer,
                read: Vec::new(),
                wrote: vec!["src/a.rs".to_owned()],
            },
        ]);
        assert_eq!(
            collisions,
            vec![Collision {
                reader,
                writer,
                path: "src/a.rs".to_owned(),
            }]
        );
    }

    #[test]
    fn reading_then_editing_the_same_file_is_not_a_collision() {
        let agent = SessionId::new();
        let collisions = detect(&[AgentTouch {
            id: agent,
            read: vec!["src/a.rs".to_owned()],
            wrote: vec!["src/a.rs".to_owned()],
        }]);
        assert!(collisions.is_empty(), "a self-edit is an ordinary edit");
    }

    #[test]
    fn disjoint_reads_and_writes_produce_no_collision() {
        let reader = SessionId::new();
        let writer = SessionId::new();
        let collisions = detect(&[
            AgentTouch {
                id: reader,
                read: vec!["src/a.rs".to_owned()],
                wrote: Vec::new(),
            },
            AgentTouch {
                id: writer,
                read: Vec::new(),
                wrote: vec!["src/b.rs".to_owned()],
            },
        ]);
        assert!(collisions.is_empty());
    }

    /// The negative case the slice requires: a stale read set must refuse a
    /// verified finish. Proven here as a pure fact — the reader whose read set
    /// contains a path a sibling wrote is the one whose finish is invalidated.
    #[test]
    fn a_stale_read_set_marks_the_reader_for_revalidation() {
        let reader = SessionId::new();
        let writer = SessionId::new();
        let collisions = detect(&[
            AgentTouch {
                id: reader,
                read: vec!["notes.md".to_owned(), "src/a.rs".to_owned()],
                wrote: Vec::new(),
            },
            AgentTouch {
                id: writer,
                read: Vec::new(),
                wrote: vec!["src/a.rs".to_owned()],
            },
        ]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].reader, reader);
        assert_eq!(collisions[0].writer, writer);
        assert_eq!(collisions[0].path, "src/a.rs");
    }

    #[test]
    fn two_writers_of_one_read_path_name_both() {
        let reader = SessionId::new();
        let first = SessionId::new();
        let second = SessionId::new();
        let collisions = detect(&[
            AgentTouch {
                id: reader,
                read: vec!["src/a.rs".to_owned()],
                wrote: Vec::new(),
            },
            AgentTouch {
                id: first,
                read: Vec::new(),
                wrote: vec!["src/a.rs".to_owned()],
            },
            AgentTouch {
                id: second,
                read: Vec::new(),
                wrote: vec!["src/a.rs".to_owned()],
            },
        ]);
        assert_eq!(
            collisions.len(),
            2,
            "one collision per writer:\n{collisions:#?}"
        );
        assert_eq!(collisions[0].writer, first);
        assert_eq!(collisions[1].writer, second);
    }
}
