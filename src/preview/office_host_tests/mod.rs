//! Tests of the Office readers (`konoma-office-core`) that need the host's Markdown renderer
//! (they render a converted document and look at what is on screen, or ask the renderer's own
//! scanners what they find). They moved here from the core crate unchanged; the helpers they
//! share with the core's own tests (`Dx`, `conv`, `md`, ...) come from the core's hidden test
//! modules (feature `test-hooks`). Each file keeps its old module name so that a path such as
//! `super::tests_docx::rendered` reads as it did.

pub use konoma_office_core::*;

mod guards;
mod tests_docx;
mod tests_docx_lists;
mod tests_docx_more;
mod tests_docx_robust;
mod tests_fidelity;
mod tests_fx34b;
mod tests_math_fx;
mod tests_odp;
mod tests_odt;
mod tests_odt_more;
mod tests_pptx;
mod tests_pptx_real;
mod tests_review_fix;
