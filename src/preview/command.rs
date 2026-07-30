// External command delegation renderer.
//
// Policy: expand the `PreviewKind::Command` template ({path} / {out}) and run a child process.
//   - render_as = "image": pass the generated artifact (PNG, etc.) to the image renderer for
//     full-screen display.
//   - detached = true: open in a separate process without blocking the TUI (e.g. video via mpv).
// If the command isn't installed or fails, don't crash — fall back to the same path as
// CanNotPreview.
//
// Stub for M0. Template expansion and child-process launch will be implemented later.
