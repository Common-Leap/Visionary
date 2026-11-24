//! Effect files staged for the game's next load of a path, and which of them it holds now.
//!
//! Plain std only: the editor's tests include this file by path and run it, since tests in the
//! plugin crate never run.

use std::collections::HashMap;

pub struct StagedFiles {
    /// path → file on the SD card and the editor's stamp for its bytes.
    staged: Option<HashMap<u64, (String, u64)>>,
    /// path → the stamp of the bytes the game last read there. None for the game's own file.
    held: Option<HashMap<u64, u64>>,
}

impl StagedFiles {
    pub const fn new() -> Self {
        Self {
            staged: None,
            held: None,
        }
    }

    /// Stage these `(path, file, stamp)` and nothing else. Whether anything was staged before.
    pub fn replace(&mut self, files: Vec<(u64, String, u64)>) -> bool {
        let files = files
            .into_iter()
            .map(|(path, file, stamp)| (path, (file, stamp)));
        self.staged
            .replace(files.collect())
            .is_some_and(|before| !before.is_empty())
    }

    /// The game loads `path`: the file to give it, if one is staged. Its own otherwise.
    pub fn read(&mut self, path: u64) -> Option<String> {
        let staged = self.staged.as_ref().and_then(|staged| staged.get(&path));
        let held = self.held.get_or_insert_with(HashMap::new);
        match staged {
            Some((_, stamp)) => held.insert(path, *stamp),
            None => held.remove(&path),
        };
        staged.map(|(file, _)| file.clone())
    }

    /// The file [`Self::read`] gave could not be: the game loaded its own.
    pub fn not_read(&mut self, path: u64) {
        self.held.get_or_insert_with(HashMap::new).remove(&path);
    }

    /// The path and stamp of every staged file the game holds, in a stable order.
    pub fn in_the_game(&self) -> Vec<(u64, u64)> {
        let mut held: Vec<_> = self.held.iter().flatten().map(|(p, s)| (*p, *s)).collect();
        held.sort_unstable();
        held
    }
}
