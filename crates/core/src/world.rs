//! The compilation environment for the editor.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use ecow::EcoString;
use rustc_hash::FxHashMap;
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
    main: FileId,
    now: Time,
}

impl StudioWorld {
    /// Creates a world rooted at the given project directory.
    ///
    /// Fonts are the embedded set plus whatever the system provides.
    pub fn new(root: PathBuf) -> Self {
        let mut fonts = FontStore::new();
        fonts.extend(fonts::embedded());
        fonts.extend(fonts::system());

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

    /// Opens a document in the editor, making it override the on-disk state.
    ///
    /// Pass `None` as the path for a document that has not been saved yet.
    pub fn open(&mut self, path: Option<&Path>, text: String) -> FileResult<FileId> {
        let id = match path {
            Some(path) => self.id_for(path)?,
            None => *SCRATCH_ID,
        };
        self.open.insert(id, Source::new(id, text));
        self.main = id;
        Ok(id)
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
        Some(self.open.get_mut(&id)?.edit(replace, with))
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
        // Populated once the Universe index is fetched in the background;
        // until then import completions simply offer nothing.
        &[]
    }

    fn files(&self) -> Vec<FileId> {
        self.open.keys().copied().collect()
    }
}
