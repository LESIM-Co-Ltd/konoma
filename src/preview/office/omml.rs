//! OMML (Office Math) -> LaTeX. **Interface only for now**: the Word reader (`docx.rs`) hands
//! every `m:oMath` / `m:oMathPara` element it finds to [`to_latex`]; the converter itself is
//! developed separately and replaces this stub. `None` means "cannot convert": the caller then
//! shows the formula's plain characters instead (principle 3: degrade, never fail).

/// Converts the raw XML of one `m:oMath` (`display = false`) or `m:oMathPara` (`display = true`)
/// element, serialised with the `m` and `w` namespace declarations on its root, to LaTeX that
/// konoma's math engine can draw. The returned string has no surrounding `$` and no newline.
pub fn to_latex(fragment: &str, display: bool) -> Option<String> {
    let _ = (fragment, display);
    None
}
