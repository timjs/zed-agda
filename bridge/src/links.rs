//! Links from names to their definitions, for go to definition.
//!
//! While loading, Agda highlights every name and, for most names, sends the
//! site of its definition. The bridge keeps those as links from a range in the
//! document to a target, and moves them along with edits until the next load,
//! like goals: edits before a link shift it, edits that touch it remove it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::protocol::HighlightingEntry;
use crate::text::Change;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A 0-based char offset in this document; it moves with edits.
    Here(usize),
    /// A 1-based code point offset in another file, as Agda read it from disk.
    File { path: PathBuf, position: usize },
}

/// A name, as a half-open range of 0-based char offsets, and its definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub start: usize,
    pub end: usize,
    pub target: Target,
    /// Whether the name is a bound variable, local to its definition.
    pub local: bool,
}

/// Links from the highlighting Agda sent while loading `file`. Entries
/// without a definition site are dropped, and repeated entries are kept once.
pub fn from_highlighting(entries: &[HighlightingEntry], file: &Path) -> Vec<Link> {
    let canonical_file = file.canonicalize().ok();
    let mut same_file: HashMap<&str, bool> = HashMap::new();
    let mut links: Vec<Link> = entries
        .iter()
        .filter_map(|entry| {
            let site = entry.definition_site.as_ref()?;
            let [from, to] = entry.range;
            let here = *same_file.entry(&site.filepath).or_insert_with(|| {
                let path = Path::new(&site.filepath);
                path == file
                    || (canonical_file.is_some() && path.canonicalize().ok() == canonical_file)
            });
            let target = if here {
                Target::Here(site.position.saturating_sub(1))
            } else {
                Target::File {
                    path: PathBuf::from(&site.filepath),
                    position: site.position,
                }
            };
            Some(Link {
                local: entry.atoms.iter().any(|atom| atom == "bound"),
                start: from.saturating_sub(1),
                end: to.saturating_sub(1),
                target,
            })
        })
        .collect();
    links.sort_by_key(|link| (link.start, link.end));
    links.dedup_by_key(|link| (link.start, link.end));
    links
}

/// Move links along with one change to the document.
pub fn adjust(links: &mut Vec<Link>, change: &Change) {
    links.retain_mut(|link| {
        if let Target::Here(target) = &mut link.target {
            if *target >= change.old_end {
                *target = target.saturating_add_signed(change.delta());
            } else if *target > change.start {
                // The definition itself was edited; point at the edit.
                *target = change.start;
            }
        }
        match change.follow(link.start, link.end) {
            Some((start, end)) => {
                (link.start, link.end) = (start, end);
                true
            }
            None => false,
        }
    });
}

/// The link at `offset`, also when the cursor sits right after a name.
pub fn link_at(links: &[Link], offset: usize) -> Option<&Link> {
    links
        .iter()
        .find(|link| link.start <= offset && offset < link.end)
        .or_else(|| links.iter().find(|link| link.end == offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::DefinitionSite;
    use crate::text::single_change;

    fn entry(range: [usize; 2], site: Option<(&str, usize)>) -> HighlightingEntry {
        HighlightingEntry {
            range,
            atoms: Vec::new(),
            definition_site: site.map(|(filepath, position)| DefinitionSite {
                filepath: filepath.into(),
                position,
            }),
        }
    }

    #[test]
    fn builds_links_from_highlighting() {
        let entries = [
            entry([45, 48], Some(("/x/Uses.agda", 37))),
            entry([1, 7], None),
            entry([43, 44], Some(("/x/Nat.agda", 24))),
            // Agda sends some entries more than once.
            entry([45, 48], Some(("/x/Uses.agda", 37))),
        ];
        let links = from_highlighting(&entries, Path::new("/x/Uses.agda"));
        assert_eq!(
            links,
            [
                Link {
                    local: false,
                    start: 42,
                    end: 43,
                    target: Target::File {
                        path: "/x/Nat.agda".into(),
                        position: 24
                    }
                },
                Link {
                    local: false,
                    start: 44,
                    end: 47,
                    target: Target::Here(36)
                },
            ]
        );
    }

    #[test]
    fn follows_edits() {
        let old = "two : N\ntwo = suc zero";
        let link = |start, end, target| Link {
            local: false,
            start,
            end,
            target: Target::Here(target),
        };
        let mut links = vec![link(8, 11, 0), link(14, 17, 0)];

        // Typing before both shifts them, and their target `two` on line 1.
        let new = "two : N\n\ntwo = suc zero";
        adjust(&mut links, &single_change(old, new).unwrap());
        assert_eq!(links, [link(9, 12, 0), link(15, 18, 0)]);

        // Replacing the name `suc` drops that link, keeps the one before it.
        // (Typing right after a name counts as after it, like for goals.)
        let newer = "two : N\n\ntwo = pred zero";
        adjust(&mut links, &single_change(new, newer).unwrap());
        assert_eq!(links, [link(9, 12, 0)]);
    }

    #[test]
    fn moves_targets_in_this_document() {
        let mut links = vec![Link {
            local: false,
            start: 20,
            end: 23,
            target: Target::Here(10),
        }];
        let change = Change {
            start: 2,
            old_end: 2,
            new_len: 3,
        };
        adjust(&mut links, &change);
        assert_eq!(links[0].target, Target::Here(13));
        assert_eq!((links[0].start, links[0].end), (23, 26));
    }

    #[test]
    fn finds_the_link_under_or_right_after_the_cursor() {
        let links = [Link {
            local: false,
            start: 4,
            end: 7,
            target: Target::Here(0),
        }];
        assert!(link_at(&links, 3).is_none());
        assert!(link_at(&links, 4).is_some());
        assert!(link_at(&links, 6).is_some());
        assert!(link_at(&links, 7).is_some());
        assert!(link_at(&links, 8).is_none());
    }
}
