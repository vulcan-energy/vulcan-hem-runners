// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

//! In-memory `OutputWriter`, for callers with no filesystem.
//!
//! Added for the h3 browser demo. h3 consumes HEM's CSV *output files*, so a
//! WASM build must be able to hand those files back rather than write them to
//! disk. This collects every file the engine emits into a map keyed by
//! `"<location_key>.<extension>"`.
//!
//! The engine clones its writer, so state is shared through an `Arc<Mutex<..>>`
//! rather than held per-clone; otherwise output written through a clone would
//! be silently discarded.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use hem_upstream::output_writer::OutputWriter;

/// Collects engine output in memory instead of writing it to a filesystem.
#[derive(Clone, Debug, Default)]
pub struct InMemoryOutputWriter {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    file_template: Option<String>,
}

impl InMemoryOutputWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the collected files as `filename -> contents`.
    ///
    /// Returns lossy UTF-8: HEM writes CSV, so any invalid byte indicates a
    /// problem worth surfacing to the caller rather than failing the whole run.
    pub fn into_string_map(self) -> BTreeMap<String, String> {
        let files = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        files
            .iter()
            .map(|(name, bytes)| (name.clone(), String::from_utf8_lossy(bytes).into_owned()))
            .collect()
    }

    /// Number of files collected so far.
    pub fn len(&self) -> usize {
        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Write handle appending to one entry of the shared map.
pub struct InMemoryFileWriter {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    name: String,
}

impl Write for InMemoryFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        files
            .entry(self.name.clone())
            .or_default()
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl OutputWriter for InMemoryOutputWriter {
    fn writer_for_location_key(
        &self,
        location_key: &str,
        file_extension: &str,
    ) -> anyhow::Result<impl Write> {
        let name = match &self.file_template {
            // The FHS wrapper requests its metrics without a model prefix.
            Some(template) if template == "{}.{}" => format!("{location_key}.{file_extension}"),
            Some(template) if !template.is_empty() => {
                format!("{template}__{location_key}.{file_extension}")
            }
            _ => format!("{location_key}.{file_extension}"),
        };
        // Ensure the entry exists even if nothing is written to it, so callers
        // can tell "empty file" from "file never produced".
        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(name.clone())
            .or_default();
        Ok(InMemoryFileWriter {
            files: Arc::clone(&self.files),
            name,
        })
    }

    fn with_file_template(&self, file_template: String) -> Self {
        Self {
            files: Arc::clone(&self.files),
            file_template: Some(file_template),
        }
    }
}
