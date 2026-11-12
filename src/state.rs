//! Small shared UI state exposed as a GPUI global, so the editor can cheaply
//! check whether a modal is open and refuse input while one is.

#[derive(Default)]
pub struct UiState {
    pub modal_open: bool,
}

impl gpui::Global for UiState {}
