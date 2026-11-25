//! A file system in memory that keeps what was synced apart from what
//! was not, for the record's crash tests (D2 of #142's review).
//!
//! Each file has its bytes as written and as synced; each directory has
//! its entries as they are and as synced. A power cut ([`Model::cut`])
//! makes a new file system from the synced images, with what was not
//! synced kept, lost or (for a file's bytes) torn, as [`Cut`] says.
//!
//! A call can be made to fail ([`Model::fail_at`]): it changes nothing
//! and returns an error. A file whose sync failed is not synced by a
//! later sync until it is written again, as Linux does: the failed
//! write-back marks its pages clean, and the next `fsync` has nothing to
//! write.
//!
//! The model's directories are kept whole by one sync of the directory,
//! as a journalling file system keeps its metadata; a renamed file's
//! entry is kept apart from its bytes, which is what makes a rename
//! before the file's sync come back empty.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use super::Fs;

/// What a power cut does with what was not synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    /// A file's bytes not synced.
    pub bytes: Bytes,
    /// A directory's entries not synced.
    pub entries: Kept,
}

/// What becomes of a file's bytes not synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bytes {
    /// They are lost: the file holds what was synced (none, for a file
    /// never synced).
    Lost,
    /// They are kept.
    Kept,
    /// The first half of them is kept, and no more.
    Torn,
}

/// Whether what was not synced is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// Lost.
    Lost,
    /// Kept.
    Kept,
}

impl Cut {
    /// Every cut.
    pub const ALL: [Cut; 6] = [
        Cut::new(Bytes::Lost, Kept::Lost),
        Cut::new(Bytes::Lost, Kept::Kept),
        Cut::new(Bytes::Kept, Kept::Lost),
        Cut::new(Bytes::Kept, Kept::Kept),
        Cut::new(Bytes::Torn, Kept::Lost),
        Cut::new(Bytes::Torn, Kept::Kept),
    ];

    const fn new(bytes: Bytes, entries: Kept) -> Cut {
        Cut { bytes, entries }
    }
}

/// A file or a directory.
#[derive(Debug, Clone)]
enum Node {
    File {
        bytes: Vec<u8>,
        synced: Vec<u8>,
        /// Its last sync failed, and nothing was written since.
        clean_but_lost: bool,
    },
    Dir {
        entries: BTreeMap<String, usize>,
        synced: BTreeMap<String, usize>,
    },
}

/// The file system's state.
#[derive(Debug, Clone)]
struct State {
    /// Node 0 is the root, which is always there.
    nodes: Vec<Node>,
    /// The calls made so far, of every kind but `read`, `list` and
    /// `lock`.
    calls: usize,
    /// The call that fails, counted as [`State::calls`].
    fail_at: Option<usize>,
    /// What each call was, in order.
    log: Vec<String>,
    /// The state after each call that changes something, for the cuts.
    after: Option<Vec<(String, Box<State>)>>,
}

/// The locks held, shared by a model and the models cut from it (a power
/// cut does not release a lock of the test's own process, but each test
/// takes its locks again on the model it cuts).
type Locks = Rc<RefCell<BTreeSet<PathBuf>>>;

/// A file system in memory ([module docs](self)).
#[derive(Debug, Clone)]
pub struct Model {
    state: Rc<RefCell<State>>,
    locks: Locks,
}

/// A lock of the model: dropped, it is released.
#[derive(Debug)]
pub struct ModelLock {
    path: PathBuf,
    locks: Locks,
}

impl Drop for ModelLock {
    fn drop(&mut self) {
        self.locks.borrow_mut().remove(&self.path);
    }
}

impl Default for Model {
    fn default() -> Self {
        Model {
            state: Rc::new(RefCell::new(State {
                nodes: vec![Node::Dir {
                    entries: BTreeMap::new(),
                    synced: BTreeMap::new(),
                }],
                calls: 0,
                fail_at: None,
                log: Vec::new(),
                after: None,
            })),
            locks: Locks::default(),
        }
    }
}

/// The parts of an absolute path.
fn parts(path: &Path) -> io::Result<Vec<String>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => match part.to_str() {
                Some(part) => parts.push(part.to_string()),
                None => return Err(io::ErrorKind::InvalidInput.into()),
            },
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(parts)
}

impl State {
    /// The node at `parts`.
    fn find(&self, parts: &[String]) -> io::Result<usize> {
        let mut at = 0;
        for part in parts {
            match &self.nodes[at] {
                Node::Dir { entries, .. } => match entries.get(part) {
                    Some(&next) => at = next,
                    None => return Err(io::ErrorKind::NotFound.into()),
                },
                Node::File { .. } => return Err(io::ErrorKind::NotADirectory.into()),
            }
        }
        Ok(at)
    }

    /// The directory that holds `path`'s entry, and the entry's name.
    fn parent(&self, path: &Path) -> io::Result<(usize, String)> {
        let mut parts = parts(path)?;
        let name = parts.pop().ok_or(io::ErrorKind::InvalidInput)?;
        let dir = self.find(&parts)?;
        match self.nodes[dir] {
            Node::Dir { .. } => Ok((dir, name)),
            Node::File { .. } => Err(io::ErrorKind::NotADirectory.into()),
        }
    }

    fn entries(&mut self, dir: usize) -> &mut BTreeMap<String, usize> {
        match &mut self.nodes[dir] {
            Node::Dir { entries, .. } => entries,
            Node::File { .. } => unreachable!("a parent is a directory"),
        }
    }

    /// Count a call that changes something, and fail it where it is the
    /// one to fail.
    fn call(&mut self, what: String) -> io::Result<()> {
        let n = self.calls;
        self.calls += 1;
        if self.fail_at == Some(n) {
            self.log.push(format!("{what}: failed"));
            return Err(io::Error::other(format!(
                "{what}: an error made by the test"
            )));
        }
        self.log.push(what);
        Ok(())
    }

    /// Keep the state after a call, where the cuts are taken.
    fn keep(&mut self) {
        if let Some(mut after) = self.after.take() {
            let what = self.log.last().cloned().unwrap_or_default();
            after.push((what, Box::new(self.clone())));
            self.after = Some(after);
        }
    }

    /// What a power cut leaves.
    fn cut(&self, cut: Cut) -> State {
        let mut nodes = self.nodes.clone();
        for node in &mut nodes {
            match node {
                Node::File {
                    bytes,
                    synced,
                    clean_but_lost,
                } => {
                    let kept = match cut.bytes {
                        Bytes::Lost => synced.clone(),
                        Bytes::Kept => bytes.clone(),
                        Bytes::Torn if bytes == synced => bytes.clone(),
                        Bytes::Torn => bytes[..bytes.len() / 2].to_vec(),
                    };
                    *bytes = kept.clone();
                    *synced = kept;
                    *clean_but_lost = false;
                }
                Node::Dir { entries, synced } => {
                    if cut.entries == Kept::Lost {
                        *entries = synced.clone();
                    }
                    *synced = entries.clone();
                }
            }
        }
        State {
            nodes,
            calls: 0,
            fail_at: None,
            log: Vec::new(),
            after: None,
        }
    }
}

impl Model {
    /// Make the call numbered `n` (from the first, 0) fail.
    pub fn fail_at(&self, n: Option<usize>) {
        self.state.borrow_mut().fail_at = n;
    }

    /// The calls that change something made so far.
    pub fn calls(&self) -> usize {
        self.state.borrow().calls
    }

    /// What each call was.
    pub fn log(&self) -> Vec<String> {
        self.state.borrow().log.clone()
    }

    /// Keep the state after each call from now on, for [`Model::cuts`].
    pub fn keep_each(&self) {
        self.state.borrow_mut().after = Some(Vec::new());
    }

    /// The models a power cut after each call since [`Model::keep_each`]
    /// leaves, with the call each followed, for each of `cuts`.
    pub fn cuts(&self) -> Vec<(String, Cut, Model)> {
        let state = self.state.borrow();
        let mut out = Vec::new();
        for (what, after) in state.after.iter().flatten() {
            for cut in Cut::ALL {
                out.push((what.clone(), cut, self.with(after.cut(cut))));
            }
        }
        out
    }

    /// The model a power cut now leaves.
    pub fn cut(&self, cut: Cut) -> Model {
        let state = self.state.borrow().cut(cut);
        self.with(state)
    }

    fn with(&self, state: State) -> Model {
        Model {
            state: Rc::new(RefCell::new(state)),
            locks: Locks::default(),
        }
    }

    /// Make the directories of `path`, synced: what the test begins on.
    pub fn base(&self, path: &Path) {
        let mut state = self.state.borrow_mut();
        let mut at = 0;
        for part in parts(path).unwrap_or_default() {
            let next = match &state.nodes[at] {
                Node::Dir { entries, .. } => entries.get(&part).copied(),
                Node::File { .. } => return,
            };
            at = match next {
                Some(next) => next,
                None => {
                    let node = state.nodes.len();
                    state.nodes.push(Node::Dir {
                        entries: BTreeMap::new(),
                        synced: BTreeMap::new(),
                    });
                    if let Node::Dir { entries, synced } = &mut state.nodes[at] {
                        entries.insert(part.clone(), node);
                        synced.insert(part, node);
                    }
                    node
                }
            };
        }
    }
}

impl Fs for Model {
    type Lock = ModelLock;

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let state = self.state.borrow();
        let node = state.find(&parts(path)?)?;
        match &state.nodes[node] {
            Node::File { bytes, .. } => Ok(bytes.clone()),
            Node::Dir { .. } => Err(io::ErrorKind::IsADirectory.into()),
        }
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let (dir, name) = state.parent(path)?;
        if state.entries(dir).contains_key(&name) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        state.call(format!("create_dir {}", path.display()))?;
        let node = state.nodes.len();
        state.nodes.push(Node::Dir {
            entries: BTreeMap::new(),
            synced: BTreeMap::new(),
        });
        state.entries(dir).insert(name, node);
        state.keep();
        Ok(())
    }

    fn write(&self, path: &Path, new: &[u8]) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let (dir, name) = state.parent(path)?;
        state.call(format!("write {} ({} bytes)", path.display(), new.len()))?;
        match state.entries(dir).get(&name).copied() {
            Some(node) => match &mut state.nodes[node] {
                Node::File {
                    bytes,
                    clean_but_lost,
                    ..
                } => {
                    *bytes = new.to_vec();
                    *clean_but_lost = false;
                }
                Node::Dir { .. } => return Err(io::ErrorKind::IsADirectory.into()),
            },
            None => {
                let node = state.nodes.len();
                state.nodes.push(Node::File {
                    bytes: new.to_vec(),
                    synced: Vec::new(),
                    clean_but_lost: false,
                });
                state.entries(dir).insert(name, node);
            }
        }
        state.keep();
        Ok(())
    }

    fn sync_file(&self, path: &Path) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let node = state.find(&parts(path)?)?;
        let failed = state.call(format!("sync_file {}", path.display()));
        match &mut state.nodes[node] {
            Node::File {
                bytes,
                synced,
                clean_but_lost,
            } => {
                if failed.is_err() {
                    *clean_but_lost = true;
                } else if !*clean_but_lost {
                    *synced = bytes.clone();
                }
            }
            Node::Dir { .. } => return Err(io::ErrorKind::IsADirectory.into()),
        }
        failed?;
        state.keep();
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let (from_dir, from_name) = state.parent(from)?;
        let (to_dir, to_name) = state.parent(to)?;
        if !state.entries(from_dir).contains_key(&from_name) {
            return Err(io::ErrorKind::NotFound.into());
        }
        state.call(format!("rename {} {}", from.display(), to.display()))?;
        if let Some(node) = state.entries(from_dir).remove(&from_name) {
            state.entries(to_dir).insert(to_name, node);
        }
        state.keep();
        Ok(())
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let node = state.find(&parts(path)?)?;
        state.call(format!("sync_dir {}", path.display()))?;
        match &mut state.nodes[node] {
            Node::Dir { entries, synced } => *synced = entries.clone(),
            Node::File { .. } => return Err(io::ErrorKind::NotADirectory.into()),
        }
        state.keep();
        Ok(())
    }

    fn unlink(&self, path: &Path) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        let (dir, name) = state.parent(path)?;
        if !state.entries(dir).contains_key(&name) {
            return Err(io::ErrorKind::NotFound.into());
        }
        state.call(format!("unlink {}", path.display()))?;
        state.entries(dir).remove(&name);
        state.keep();
        Ok(())
    }

    fn list(&self, path: &Path) -> io::Result<Vec<String>> {
        let state = self.state.borrow();
        let node = state.find(&parts(path)?)?;
        match &state.nodes[node] {
            Node::Dir { entries, .. } => Ok(entries.keys().cloned().collect()),
            Node::File { .. } => Err(io::ErrorKind::NotADirectory.into()),
        }
    }

    fn lock(&self, path: &Path) -> io::Result<ModelLock> {
        if !self.locks.borrow_mut().insert(path.to_path_buf()) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(ModelLock {
            path: path.to_path_buf(),
            locks: self.locks.clone(),
        })
    }
}
