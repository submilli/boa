//! Declaration-ordered bindings with a name index for larger scopes.
use super::Binding;
use boa_string::JsString;
use std::{collections::BTreeMap, fmt};

// Most scopes are small. Keep their lookup allocation-free and avoid maintaining
// an index until repeated linear scans cost more than tree lookups.
const INDEX_THRESHOLD: usize = 32;

#[derive(Default, PartialEq)]
pub(super) struct Bindings {
    entries: Vec<Binding>,
    // Positions in `entries`, not Binding::index: escape analysis can renumber
    // binding slots, but declarations are append-only and their names never change.
    // A tree bounds lookup comparisons without entropy or collision-prone hashing
    // of page-controlled names. Boxing keeps unindexed scopes one pointer larger.
    #[allow(clippy::box_collection)] // Keep the common unindexed scope one pointer larger.
    by_name: Option<Box<BTreeMap<JsString, usize>>>,
}

impl fmt::Debug for Bindings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.entries.fmt(f)
    }
}

impl Bindings {
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn first(&self) -> Option<&Binding> {
        self.entries.first()
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, Binding> {
        self.entries.iter()
    }

    pub(super) fn iter_mut(&mut self) -> std::slice::IterMut<'_, Binding> {
        self.entries.iter_mut()
    }

    pub(super) fn find(&self, name: &JsString) -> Option<&Binding> {
        let position = self.position(name)?;
        self.entries.get(position)
    }

    pub(super) fn find_mut(&mut self, name: &JsString) -> Option<&mut Binding> {
        let position = self.position(name)?;
        self.entries.get_mut(position)
    }

    fn position(&self, name: &JsString) -> Option<usize> {
        match &self.by_name {
            Some(index) => index.get(name).copied(),
            None => self
                .entries
                .iter()
                .position(|binding| &binding.name == name),
        }
    }

    // The scope's creation methods check for duplicates before appending.
    pub(super) fn push(&mut self, binding: Binding) {
        if let Some(index) = &mut self.by_name {
            index.insert(binding.name.clone(), self.entries.len());
        }
        self.entries.push(binding);
        if self.by_name.is_none() && self.entries.len() > INDEX_THRESHOLD {
            self.by_name = Some(Box::new(
                self.entries
                    .iter()
                    .enumerate()
                    .map(|(position, binding)| (binding.name.clone(), position))
                    .collect(),
            ));
        }
    }
}
