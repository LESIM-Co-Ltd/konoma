//! The standard file plugins shared by konoma hosts.

use std::sync::Arc;

use konoma_plugin::Plugin;

/// The real implementations in registration order. SVG precedes the general image MIME binding;
/// math handles embedded formulas only and keeps its place alongside Markdown and Mermaid.
pub fn plugins() -> Vec<Arc<dyn Plugin>> {
    vec![
        Arc::new(konoma_markdown::MarkdownPlugin),
        Arc::new(konoma_mermaid::MermaidPlugin),
        Arc::new(konoma_math::MathPlugin),
        Arc::new(konoma_media::SvgPlugin),
        Arc::new(konoma_media::ImagePlugin),
        Arc::new(konoma_table::CsvPlugin),
        Arc::new(konoma_office::SpreadsheetPlugin),
        Arc::new(konoma_office::DocumentPlugin),
        Arc::new(konoma_office::PresentationPlugin),
        Arc::new(konoma_table::ArchivePlugin),
        Arc::new(konoma_code::CodePlugin),
        Arc::new(konoma_media::VideoPlugin),
        Arc::new(konoma_media::PdfPlugin),
    ]
}
