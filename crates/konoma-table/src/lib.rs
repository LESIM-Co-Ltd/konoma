//! konoma's table plugins: `csv` (CSV and TSV files) and `archive` (zip, tar and tar.gz listings).
//!
//! Both read a file into a grid and return it as a [`konoma_plugin::view::TableView`]; the host
//! draws the grid, moves the cell cursor, searches and copies. The reading itself is in
//! [`table`] and [`archive`].

pub mod archive;
pub mod table;

use std::path::Path;
use std::sync::Arc;

use konoma_plugin::view::{Grid, TableView, View, ViewReq};
use konoma_plugin::{
    t, Bind, Failure, Input, LaunchMode, Launcher, Manifest, OpenCx, Plugin, Session, SettingDecl,
    Value, Verb, VerbDecl, ViewCx,
};

pub use archive::ArchiveKind;
pub use table::TableData;

/// The id of the CSV/TSV plugin.
pub const CSV_ID: &str = "csv";
/// The id of the archive plugin.
pub const ARCHIVE_ID: &str = "archive";

/// `edit` opens the user's editor in the terminal; `open` hands the file to the OS default
/// application as a GUI program.
fn default_verbs(m: Manifest) -> Manifest {
    m.verb(
        VerbDecl::new(Verb::Edit)
            .launcher(Launcher::Editor)
            .mode(LaunchMode::Terminal)
            .line_token(),
    )
    .verb(
        VerbDecl::new(Verb::Open)
            .launcher(Launcher::OsDefault)
            .mode(LaunchMode::Gui),
    )
}

/// The declarations of the `csv` plugin.
pub fn csv_manifest() -> Manifest {
    default_verbs(
        Manifest::new(CSV_ID, env!("CARGO_PKG_VERSION"))
            .name(t!(en: "CSV / TSV", ja: "CSV / TSV"))
            .bind(Bind::files(["*.csv", "*.tsv"]))
            .setting(SettingDecl::new(
                "rainbow",
                Value::Bool(true),
                t!(en: "Color the columns", ja: "列を色分けする"),
            )),
    )
}

/// The declarations of the `archive` plugin.
pub fn archive_manifest() -> Manifest {
    default_verbs(
        Manifest::new(ARCHIVE_ID, env!("CARGO_PKG_VERSION"))
            .name(t!(en: "Archive", ja: "アーカイブ"))
            .bind(Bind::files(["*.{zip,tar,tgz}", "*.tar.gz"])),
    )
}

/// The field separator of a CSV/TSV input: the note from the binding wins (`"tsv"` is the tab, any
/// other note is the comma), otherwise the extension decides.
pub fn delimiter(hint: Option<&str>, path: &Path) -> u8 {
    match hint {
        Some("tsv") => b'\t',
        Some(_) => b',',
        None => {
            let tsv = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
            if tsv {
                b'\t'
            } else {
                b','
            }
        }
    }
}

/// The path of a file input, or why the input is not one.
fn file_of(input: &Input) -> Result<(&Path, Option<&str>), Failure> {
    match input {
        Input::File { path, hint } => Ok((path, hint.as_deref())),
        _ => Err(Failure::unsupported(t!(
            en: "this plugin opens files only",
            ja: "このプラグインはファイルだけを開きます",
        ))),
    }
}

/// An opened table: the grid is shared with every view it returns.
struct TableSession {
    grid: Arc<dyn Grid>,
}

impl Session for TableSession {
    fn view(&mut self, _req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        Ok(View::Table(TableView {
            grid: Arc::clone(&self.grid),
            // The host titles a table with the file's path and the cursor position.
            title: None,
            rainbow: cx.settings.bool("rainbow", true),
            addressed: false,
        }))
    }
}

/// The CSV/TSV plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct CsvPlugin;

impl Plugin for CsvPlugin {
    fn manifest(&self) -> Manifest {
        csv_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let (path, hint) = file_of(&input)?;
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        let data = table::parse(path, delimiter(hint, path)).map_err(|e| {
            Failure::corrupt(t!(en: "cannot read the table", ja: "表を読めません"))
                .with_detail(format!("{e:#}"))
        })?;
        Ok(Box::new(TableSession {
            grid: Arc::new(data),
        }))
    }
}

/// The archive plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArchivePlugin;

impl Plugin for ArchivePlugin {
    fn manifest(&self) -> Manifest {
        archive_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let (path, _) = file_of(&input)?;
        let Some(kind) = ArchiveKind::from_path(path) else {
            return Err(Failure::unsupported(t!(
                en: "not a zip, tar or tar.gz file",
                ja: "zip・tar・tar.gz ではありません",
            )));
        };
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        // Listing relies on third-party crates (zip, tar). Even an unlikely panic inside them is
        // swallowed here and becomes a failure, which the host shows as "cannot list".
        let listed = konoma_base::panic::catch_silent(|| archive::list(path, kind));
        match listed {
            Some(Ok(data)) => Ok(Box::new(TableSession {
                grid: Arc::new(data),
            })),
            Some(Err(e)) => Err(Failure::corrupt(
                t!(en: "cannot list the archive", ja: "アーカイブを一覧できません"),
            )
            .with_detail(format!("{e:#}"))),
            None => Err(Failure::corrupt(
                t!(en: "cannot list the archive", ja: "アーカイブを一覧できません"),
            )
            .with_detail("panic while listing")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_plugin::testing::{check_manifest, open_file};
    use konoma_test_support::unique_tmp;

    fn grid_of(session: &mut dyn Session, rainbow: Option<bool>) -> (Arc<dyn Grid>, bool) {
        let mut settings = konoma_plugin::Settings::new();
        if let Some(r) = rainbow {
            settings.set("rainbow", Value::Bool(r));
        }
        let cx = ViewCx::new(&settings, "en");
        match session.view(&ViewReq::default(), &cx).unwrap() {
            View::Table(t) => (t.grid, t.rainbow),
            other => panic!("expected a table view: {other:?}"),
        }
    }

    #[test]
    fn manifests_are_clean_and_bind_the_old_globs() {
        for plugin in [&CsvPlugin as &dyn Plugin, &ArchivePlugin] {
            assert!(check_manifest(plugin).is_empty());
            let m = plugin.manifest();
            assert_eq!(m.api, konoma_plugin::ApiVersion::CURRENT);
            assert_eq!(m.verbs.len(), 2, "{}", m.id);
        }
        assert_eq!(CsvPlugin.manifest().id, CSV_ID);
        assert_eq!(ArchivePlugin.manifest().id, ARCHIVE_ID);
    }

    #[test]
    fn delimiter_follows_the_note_then_the_extension() {
        let csv = Path::new("a.csv");
        let tsv = Path::new("a.TSV");
        assert_eq!(delimiter(None, csv), b',');
        assert_eq!(delimiter(None, tsv), b'\t');
        assert_eq!(delimiter(Some("tsv"), csv), b'\t');
        // Any other note is the comma, even on a `.tsv` name (an old `builtin = "csv"` rule).
        assert_eq!(delimiter(Some("csv"), tsv), b',');
    }

    #[test]
    fn csv_opens_to_a_grid_and_the_rainbow_setting_reaches_the_view() {
        let dir = unique_tmp("konoma_table_plugin");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.csv");
        std::fs::write(&p, "h1,h2\na,b\nc,d\n").unwrap();
        let mut s = open_file(&CsvPlugin, &p).unwrap();
        let (g, rainbow) = grid_of(&mut *s, None);
        assert!(rainbow, "rainbow is on by default");
        assert_eq!((g.nrows(), g.ncols()), (2, 2));
        assert_eq!(g.header(1).as_deref(), Some("h2"));
        assert_eq!(g.header(2), None);
        assert_eq!(g.cell(1, 0), "c");
        assert_eq!(g.cell_str(1, 0), Some("c"));
        assert_eq!(g.cell(9, 9), "");
        assert!(!g.truncated());
        let (_, rainbow) = grid_of(&mut *s, Some(false));
        assert!(!rainbow);
    }

    #[test]
    fn a_tsv_name_or_note_picks_the_tab() {
        let dir = unique_tmp("konoma_table_plugin_tsv");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.tsv");
        std::fs::write(&p, "x\ty\n1\t2\n").unwrap();
        let mut s = open_file(&CsvPlugin, &p).unwrap();
        assert_eq!(grid_of(&mut *s, None).0.cell(0, 1), "2");
    }

    #[test]
    fn failures_are_reported_not_panicked() {
        let dir = unique_tmp("konoma_table_plugin_fail");
        std::fs::create_dir_all(&dir).unwrap();
        // A missing file.
        assert!(open_file(&CsvPlugin, &dir.join("nope.csv")).is_err());
        // Not an archive name, and a corrupt archive.
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "x").unwrap();
        let err = open_file(&ArchivePlugin, &txt).err().unwrap();
        assert_eq!(err.kind, konoma_plugin::FailureKind::Unsupported);
        let bad = dir.join("bad.zip");
        std::fs::write(&bad, b"not a zip file at all").unwrap();
        let err = open_file(&ArchivePlugin, &bad).err().unwrap();
        assert_eq!(err.kind, konoma_plugin::FailureKind::Corrupt);
        // A content input is not a file.
        let err = CsvPlugin
            .open(
                Input::Embed {
                    kind: "x".into(),
                    source: String::new(),
                    display: false,
                },
                &OpenCx::bare(),
            )
            .err()
            .unwrap();
        assert_eq!(err.kind, konoma_plugin::FailureKind::Unsupported);
    }

    #[test]
    fn a_cancelled_open_stops_before_reading() {
        let dir = unique_tmp("konoma_table_plugin_cancel");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.csv");
        std::fs::write(&p, "a\n1\n").unwrap();
        let settings = konoma_plugin::Settings::new();
        let cx = OpenCx::new(&settings, "en").with_cancel(konoma_plugin::Cancel::from_fn(|| true));
        let err = CsvPlugin
            .open(
                Input::File {
                    path: p,
                    hint: None,
                },
                &cx,
            )
            .err()
            .unwrap();
        assert_eq!(err.kind, konoma_plugin::FailureKind::Cancelled);
    }

    #[test]
    fn an_archive_opens_to_name_size_modified() {
        use std::io::Write as _;
        let dir = unique_tmp("konoma_table_plugin_zip");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.zip");
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("one.txt", opts).unwrap();
        zw.write_all(b"hello").unwrap();
        zw.finish().unwrap();
        let mut s = open_file(&ArchivePlugin, &p).unwrap();
        let (g, _) = grid_of(&mut *s, None);
        assert_eq!(g.header(0).as_deref(), Some("Name"));
        assert_eq!(g.header(2).as_deref(), Some("Modified"));
        assert_eq!((g.nrows(), g.ncols()), (1, 3));
        assert_eq!(g.cell(0, 0), "one.txt");
        assert_eq!(g.cell(0, 1), "5 B");
    }
}
