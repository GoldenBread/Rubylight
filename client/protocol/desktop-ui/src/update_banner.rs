//! Pure egui update availability / download progress banner, decoupled from HTTP and
//! `self_update` OS installers.

use crate::strings::trf;

/// Everything the banner needs to draw on one frame.
#[derive(Clone, Debug)]
pub struct UpdateBannerState {
    pub version: String,
    pub url: String,
    pub progress: Option<(u64, u64)>,
    pub failure_text: Option<String>,
    pub can_install: bool,
    pub held: bool,
}

/// What the user clicked on the update banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateBannerAction {
    UpdateNow,
    Cancel,
    OpenDownloadPage(String),
    Close,
    SkipVersion,
}

/// Draws the update banner over the bottom-right corner of the launcher, returning any clicked action.
pub fn show(ctx: &egui::Context, state: &UpdateBannerState) -> Option<UpdateBannerAction> {
    let mut action = None;
    egui::Area::new(egui::Id::unique("update-notice"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-24.0, -24.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(*crate::theme::SURFACE_HIGHEST)
                .corner_radius(egui::CornerRadius::same(14))
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_max_width(360.0);
                    if let Some((received, size)) = state.progress {
                        ui.label(trf("Downloading Nightjar {}", &[&state.version]));
                        ui.add(
                            egui::ProgressBar::new(received as f32 / size.max(1) as f32)
                                .desired_width(280.0)
                                .show_percentage(),
                        );
                        if crate::ui::quiet_button(ui, "Cancel").clicked() {
                            action = Some(UpdateBannerAction::Cancel);
                        }
                        return;
                    }
                    ui.label(trf("Nightjar {} is available.", &[&state.version]));
                    if let Some(failure) = &state.failure_text {
                        ui.colored_label(ui.visuals().error_fg_color, failure);
                    }
                    ui.horizontal(|ui| {
                        if state.can_install
                            && ui
                                .add_enabled_ui(!state.held, |ui| crate::ui::primary_button(ui, "Update now"))
                                .inner
                                .clicked()
                        {
                            action = Some(UpdateBannerAction::UpdateNow);
                        }
                        if crate::ui::quiet_button(ui, "Open download page").clicked() {
                            action = Some(UpdateBannerAction::OpenDownloadPage(state.url.clone()));
                        }
                        if crate::ui::quiet_button(ui, "Not now").clicked() {
                            action = Some(UpdateBannerAction::Close);
                        }
                    });
                    ui.horizontal(|ui| {
                        if crate::ui::quiet_button(ui, "Skip this version").clicked() {
                            action = Some(UpdateBannerAction::SkipVersion);
                        }
                    });
                });
        });
    action
}
