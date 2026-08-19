//! The compilation environment for the editor.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use ecow::EcoString;
use rustc_hash::{FxHashMap, FxHashSet};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::package::PackageSpec;
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::datetime::Time;
use typst_kit::downloader::SystemDownloader;
use typst_kit::files::{FileStore, FsRoot, SystemFiles};
use typst_kit::fonts::{self, FontStore};
use typst_kit::packages::SystemPackages;

/// The file id used for a document that has no path on disk yet.
static SCRATCH_ID: LazyLock<FileId> = LazyLock::new(|| {
    FileId::unique(RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new("<scratch>").unwrap(),
    ))
});

/// The environment in which the editor compiles documents.
///
/// Files that are open in the editor live in `open` as [`Source`] objects and
/// take precedence over the on-disk state. This is what makes typing cheap:
/// [`Source::edit`] reparses incrementally and keeps span numbers stable, so
/// `comemo` can reuse most of the previous compilation.
pub struct StudioWorld {
    library: LazyHash<Library>,
    fonts: FontStore,
    root: PathBuf,
    files: FileStore<SystemFiles>,
    open: FxHashMap<FileId, Source>,
    /// Documents edited since they were last read from or written to disk.
    dirty: FxHashSet<FileId>,
    /// Known Universe packages, for import completion.
    packages: Vec<(PackageSpec, Option<EcoString>)>,
    main: FileId,
    now: Time,
}

/// Whether the system's fonts are available in addition to the embedded ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemFonts {
    /// Search installed fonts too, so documents can name what the user has.
    Include,
    /// Use only the embedded fonts, so output does not depend on the machine.
    Exclude,
}

impl StudioWorld {
    /// Creates a world rooted at the given project directory, with system fonts
    /// available.
    pub fn new(root: PathBuf) -> Self {
        Self::with_fonts(root, SystemFonts::Include)
    }

    /// Creates a world with an explicit font policy.
    ///
    /// The embedded fonts are always registered first, so a document that names
    /// one of them renders identically on every machine even when a font of the
    /// same name is installed locally. System fonts extend that set rather than
    /// override it.
    pub fn with_fonts(root: PathBuf, system: SystemFonts) -> Self {
        let mut fonts = FontStore::new();
        fonts.extend(fonts::embedded());
        if system == SystemFonts::Include {
            fonts.extend(fonts::system());
        }

        let packages = SystemPackages::new(SystemDownloader::new(concat!(
            "typst-studio/",
            env!("CARGO_PKG_VERSION")
        )));

        Self {
            library: LazyHash::new(Library::builder().build()),
            fonts,
            files: FileStore::new(SystemFiles::new(FsRoot::new(root.clone()), packages)),
            root,
            open: FxHashMap::default(),
            dirty: FxHashSet::default(),
            packages: Vec::new(),
            main: *SCRATCH_ID,
            now: Time::system(),
        }
    }

    /// The project root on disk.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The id of the document that is currently being compiled.
    pub fn main_id(&self) -> FileId {
        self.main
    }

    /// Resolves a path inside the project root to a file id.
    pub fn id_for(&self, path: &Path) -> FileResult<FileId> {
        let vpath = VirtualPath::virtualize(self.root(), path)
            .map_err(|_| FileError::NotFound(path.into()))?;
        Ok(RootedPath::new(VirtualRoot::Project, vpath).intern())
    }

    /// Resolves a path relative to the project root to a file id.
    pub fn id_for_relative(&self, path: &str) -> FileResult<FileId> {
        self.id_for(&self.root.join(path))
    }

    /// Supplies the package list used for import completion.
    ///
    /// Fetched in the background, so completion works without it and improves
    /// once it arrives.
    pub fn set_packages(&mut self, packages: Vec<(PackageSpec, Option<EcoString>)>) {
        self.packages = packages;
    }

    /// The path of a file relative to the project root, for display.
    pub fn relative_path(&self, id: FileId) -> Option<String> {
        if id == *SCRATCH_ID {
            return None;
        }
        Some(id.vpath().get_without_slash().to_string())
    }

    /// Opens a document in the editor, making it override the on-disk state.
    ///
    /// Also makes it the compiled document; call [`set_main`](Self::set_main)
    /// afterwards to edit a file that is included by another one.
    ///
    /// Pass `None` as the path for a document that has not been saved yet.
    pub fn open(&mut self, path: Option<&Path>, text: String) -> FileResult<FileId> {
        let id = match path {
            Some(path) => self.id_for(path)?,
            None => *SCRATCH_ID,
        };
        self.open.insert(id, Source::new(id, text));
        self.dirty.remove(&id);
        self.main = id;
        Ok(id)
    }

    /// Opens a file from disk without changing which document is compiled.
    pub fn open_from_disk(&mut self, path: &str) -> FileResult<FileId> {
        let id = self.id_for_relative(path)?;
        if !self.open.contains_key(&id) {
            let source = self.files.source(id)?;
            self.open.insert(id, source);
            self.dirty.remove(&id);
        }
        Ok(id)
    }

    /// Re-reads an open document from disk, discarding the in-memory copy.
    pub fn reload(&mut self, id: FileId) -> FileResult<()> {
        self.files.reset();
        let source = self.files.source(id)?;
        self.open.insert(id, source);
        self.dirty.remove(&id);
        Ok(())
    }

    /// Whether a document has unsaved changes.
    pub fn is_dirty(&self, id: FileId) -> bool {
        self.dirty.contains(&id)
    }

    /// Writes an open document back to disk.
    ///
    /// Fails for a document that has no path yet.
    pub fn save(&mut self, id: FileId) -> FileResult<()> {
        let source = self.open.get(&id).ok_or(FileError::Other(None))?;
        let path = self
            .files
            .loader()
            .resolve(id)
            .map_err(|_| FileError::NotFound(self.root.clone()))?;

        std::fs::write(&path, source.text())
            .map_err(|err| FileError::from_io(err, &path))?;
        self.dirty.remove(&id);
        Ok(())
    }

    /// The ids of all documents open in this window.
    pub fn open_documents(&self) -> Vec<FileId> {
        self.open.keys().copied().collect()
    }

    /// Closes a document, discarding any unsaved changes.
    pub fn close(&mut self, id: FileId) {
        self.open.remove(&id);
        self.dirty.remove(&id);
    }

    /// The Typst files in the project, relative to the root, sorted.
    ///
    /// Hidden directories and the usual build output are skipped; a project is
    /// a source tree, not a file manager.
    pub fn project_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        collect_typst_files(&self.root, &self.root, &mut files);
        files.sort();
        files
    }

    /// The files the last compilation read, for the file watcher.
    pub fn dependencies(&mut self) -> Vec<PathBuf> {
        let (loader, deps) = self.files.dependencies();
        deps.filter_map(|id| loader.resolve(id).ok()).collect()
    }

    /// The current text of an open document.
    pub fn source_text(&self, id: FileId) -> Option<String> {
        Some(self.open.get(&id)?.text().to_string())
    }

    /// Applies an edit to an open document and returns the range of the
    /// reparsed region.
    ///
    /// This is the hot path: it runs on every keystroke.
    pub fn edit(
        &mut self,
        id: FileId,
        replace: std::ops::Range<usize>,
        with: &str,
    ) -> Option<std::ops::Range<usize>> {
        let range = self.open.get_mut(&id)?.edit(replace, with);
        self.dirty.insert(id);
        Some(range)
    }

    /// Selects which open document is compiled.
    pub fn set_main(&mut self, id: FileId) {
        self.main = id;
    }

    /// Discards cached on-disk state in preparation for a new compilation.
    ///
    /// Open documents are deliberately kept, since they are the source of
    /// truth while the editor is running.
    pub fn reset(&mut self) {
        self.files.reset();
        self.now.reset();
    }
}

impl World for StudioWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        match self.open.get(&id) {
            Some(source) => Ok(source.clone()),
            None => self.files.source(id),
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        match self.open.get(&id) {
            Some(source) => Ok(Bytes::from_string(source.text().to_string())),
            None => self.files.file(id),
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.now.today(offset)
    }
}

impl typst_ide::IdeWorld for StudioWorld {
    fn upcast(&self) -> &dyn World {
        self
    }

    fn packages(&self) -> &[(PackageSpec, Option<EcoString>)] {
        &self.packages
    }

    fn files(&self) -> Vec<FileId> {
        self.open.keys().copied().collect()
    }
}

/// Walks the project tree, collecting Typst sources.
fn collect_typst_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }

        if path.is_dir() {
            collect_typst_files(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "typ")
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}
