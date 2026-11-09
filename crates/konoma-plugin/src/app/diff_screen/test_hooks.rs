//! Read-only inspection for integration tests, including the app-only feature matrix.
use super::{DiffPresentation, DiffScreenState};

/// Import explicitly in tests; presentation itself is internal to the app.
pub trait DiffScreenTestHooks {
    fn presentation(&self) -> DiffPresentation;
}

impl DiffScreenTestHooks for DiffScreenState {
    fn presentation(&self) -> DiffPresentation {
        DiffScreenState::presentation(self)
    }
}
