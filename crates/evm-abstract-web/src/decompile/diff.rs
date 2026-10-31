//! Markers come from typed snapshot differences and core-recorded clone/edge
//! events. Their text is always rendered alongside their presentation color.

use crate::palette;
use egui::Color32;
use evm_abstract_protocol::decompile_model::{Artifact, EntityId, StageId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Marker {
    New,
    Copied,
    Removed,
    Redirected,
    Changed,
}

impl Marker {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Copied => "Copied",
            Self::Removed => "Removed",
            Self::Redirected => "Redirected",
            Self::Changed => "Changed",
        }
    }
    pub fn color(self) -> Color32 {
        match self {
            Self::New => palette::ACCENT,
            Self::Copied => palette::PURPLE,
            Self::Removed => palette::ERROR,
            Self::Redirected => palette::WARNING,
            Self::Changed => palette::BLUE,
        }
    }
}

#[derive(Default)]
pub(super) struct Differences {
    pub before: BTreeMap<EntityId, BTreeSet<Marker>>,
    pub after: BTreeMap<EntityId, BTreeSet<Marker>>,
    pub rows: Vec<(bool, EntityId)>,
    summary: String,
}

impl Differences {
    pub fn new(artifact: &Artifact, before: StageId, after: StageId) -> Self {
        let mut result = Self::default();
        let Some(diff) = artifact.diff(before, after) else {
            return result;
        };
        for id in diff.added {
            result.after.entry(id).or_default().insert(Marker::New);
        }
        for id in diff.removed {
            result.before.entry(id).or_default().insert(Marker::Removed);
        }
        for id in diff.changed {
            result.before.entry(id).or_default().insert(Marker::Changed);
            result.after.entry(id).or_default().insert(Marker::Changed);
        }
        let mut chain = BTreeSet::new();
        let mut cursor = after;
        while cursor != before && chain.insert(cursor) {
            let Some(parent) = artifact.stage(cursor).and_then(|stage| stage.parent) else {
                break;
            };
            cursor = parent;
        }
        for event in artifact
            .events
            .iter()
            .filter(|event| chain.contains(&event.to_stage))
        {
            for copy in &event.clones {
                if let Some(markers) = result.after.get_mut(&copy.copy.entity) {
                    markers.remove(&Marker::New);
                    markers.insert(Marker::Copied);
                }
            }
            for redirect in &event.redirects {
                if artifact
                    .stage(before)
                    .is_some_and(|stage| stage.ir.edge(redirect.edge).is_some())
                {
                    result
                        .before
                        .entry(redirect.edge)
                        .or_default()
                        .insert(Marker::Redirected);
                }
                if artifact
                    .stage(after)
                    .is_some_and(|stage| stage.ir.edge(redirect.edge).is_some())
                {
                    result
                        .after
                        .entry(redirect.edge)
                        .or_default()
                        .insert(Marker::Redirected);
                }
            }
        }
        result.rows = result
            .before
            .keys()
            .map(|id| (true, *id))
            .chain(result.after.keys().map(|id| (false, *id)))
            .collect();
        result.summary = result.counts();
        result
    }

    pub fn markers(&self, id: EntityId, before: bool) -> Vec<(String, Color32)> {
        (if before { &self.before } else { &self.after })
            .get(&id)
            .into_iter()
            .flatten()
            .map(|marker| (marker.label().into(), marker.color()))
            .collect()
    }

    pub fn summary(&self) -> String {
        self.summary.clone()
    }

    fn counts(&self) -> String {
        [
            Marker::New,
            Marker::Copied,
            Marker::Removed,
            Marker::Redirected,
            Marker::Changed,
        ]
        .into_iter()
        .map(|marker| {
            let count = if marker == Marker::Removed {
                self.before.values()
            } else {
                self.after.values()
            }
            .filter(|markers| markers.contains(&marker))
            .count();
            format!("{}: {count}", marker.label())
        })
        .collect::<Vec<_>>()
        .join(" · ")
    }
}
