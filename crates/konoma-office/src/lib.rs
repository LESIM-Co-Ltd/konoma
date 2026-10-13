//! konoma's Office plugins.
//!
//! Three plugins that share one work group ([`OFFICE_WORK_GROUP`]):
//! - `spreadsheet`: a workbook (xlsx, xlsm, xltx, xltm, xlsb, xls, ods) shown as a
//!   **sheet-by-sheet table**. The sheets are the plugin's pages ([`PageLayout::Separate`]); the
//!   host asks for one at a time and draws it with its table widget.
//! - `document`: a Word or OpenDocument text file converted to Markdown with its pictures
//!   (`documents.rs`).
//! - `presentation`: a PowerPoint or OpenDocument presentation: one long document whose slides are
//!   anchored pages, as slide pictures or as text (`documents.rs`).
//!
//! The reading is in `konoma-office-core`; this crate is the plugin around it: the declarations,
//! the session that reads one sheet per `view`, the grid the host searches and copies from, and the
//! words the user reads (the cell popup, the title, the reasons a workbook cannot be shown).

mod documents;
mod spreadsheet;
mod texts;

use konoma_plugin::{t, Bind, LaunchMode, Launcher, Manifest, SettingDecl, Value, Verb, VerbDecl};

pub use documents::{
    read_converted, slide_px, slide_resource, Converted, DocKind, DocumentPlugin, OfficeSession,
    PresentationPlugin, REPR_PICTURE, REPR_TEXT, SLIDE_HEADING_LEVEL,
};
/// The failure a Word document or presentation that could not be read is shown with.
pub fn doc_failure(err: &konoma_office_core::OfficeError, kind: DocKind) -> konoma_plugin::Failure {
    texts::doc_failure_of(err, kind)
}

pub use spreadsheet::{table_view, SheetGrid, SpreadsheetPlugin};

/// The id of the spreadsheet plugin.
pub const SPREADSHEET_ID: &str = "spreadsheet";
/// The id of the Word plugin (declared here so the three Office plugins share one group).
pub const DOCUMENT_ID: &str = "document";
/// The id of the slide plugin.
pub const PRESENTATION_ID: &str = "presentation";

/// The exclusive work group of the Office plugins. Reading a workbook, a Word file or a deck can
/// take gigabytes, so the host runs one such job at a time across the group and keeps only the
/// newest waiting one (`Manifest::work_group`): holding `J` over a big workbook never piles up
/// loads.
pub const OFFICE_WORK_GROUP: &str = "office";

/// The glob of the Word files.
pub const DOCUMENT_GLOB: &str = "*.{docx,docm,dotx,dotm,odt,ott}";

/// The glob of the presentation files.
pub const PRESENTATION_GLOB: &str = "*.{pptx,pptm,ppsx,ppsm,potx,potm,odp,otp}";

/// The glob of the spreadsheet files.
pub const SPREADSHEET_GLOB: &str = "*.{xlsx,xlsm,xltx,xltm,xlsb,xls,ods}";

/// The chain `e` and `o` follow for an Office file: the Microsoft app, then LibreOffice, then the
/// OS default application. `microsoft_bundle_id` is the macOS bundle id of the Microsoft app for
/// the kind of file.
pub fn office_verbs(m: Manifest, microsoft_bundle_id: &str) -> Manifest {
    let chain = |verb: Verb| {
        VerbDecl::new(verb)
            .launcher(Launcher::MacApp {
                bundle_id: microsoft_bundle_id.to_string(),
            })
            .launcher(Launcher::MacApp {
                bundle_id: "org.libreoffice.script".to_string(),
            })
            .launcher(Launcher::Command(vec!["libreoffice".to_string()]))
            .launcher(Launcher::Command(vec!["soffice".to_string()]))
            .launcher(Launcher::OsDefault)
            .mode(LaunchMode::Gui)
    };
    m.verb(chain(Verb::Edit)).verb(chain(Verb::Open))
}

/// The declarations of the `spreadsheet` plugin.
pub fn spreadsheet_manifest() -> Manifest {
    office_verbs(
        Manifest::new(SPREADSHEET_ID, env!("CARGO_PKG_VERSION"))
            .name(t!(en: "Spreadsheet", ja: "表計算"))
            .bind(Bind::files([SPREADSHEET_GLOB]))
            .setting(SettingDecl::new(
                "rainbow",
                Value::Bool(true),
                t!(en: "Color the columns", ja: "列を色分けする"),
            ))
            .work_group(OFFICE_WORK_GROUP),
        "com.microsoft.Excel",
    )
}

/// The declarations of the `document` plugin.
pub fn document_manifest() -> Manifest {
    office_verbs(
        Manifest::new(DOCUMENT_ID, env!("CARGO_PKG_VERSION"))
            .name(t!(en: "Word document", ja: "Word 文書"))
            .bind(Bind::files([DOCUMENT_GLOB]))
            .work_group(OFFICE_WORK_GROUP),
        "com.microsoft.Word",
    )
}

/// The declarations of the `presentation` plugin.
pub fn presentation_manifest() -> Manifest {
    office_verbs(
        Manifest::new(PRESENTATION_ID, env!("CARGO_PKG_VERSION"))
            .name(t!(en: "Presentation", ja: "プレゼンテーション"))
            .bind(Bind::files([PRESENTATION_GLOB]))
            .work_group(OFFICE_WORK_GROUP),
        "com.microsoft.Powerpoint",
    )
}
