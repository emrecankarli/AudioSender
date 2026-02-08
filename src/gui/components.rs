use eframe::egui;

/// A styled button with an icon
pub fn icon_button(ui: &mut egui::Ui, icon: &str, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        ui.button(format!("{} {}", icon, text))
    }).inner
}

/// A status indicator with a colored dot
pub fn status_indicator(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("●").color(color).size(16.0));
        ui.label(text);
    });
}

/// An audio level meter bar
pub fn audio_level_meter(ui: &mut egui::Ui, level: f32, width: f32, height: f32) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(width, height),
        egui::Sense::hover()
    );
    
    // Background
    painter.rect_filled(
        response.rect,
        4.0,
        egui::Color32::from_gray(40)
    );
    
    // Level bar
    let level_width = width * level.clamp(0.0, 1.0);
    let color = if level < 0.7 {
        egui::Color32::from_rgb(0, 200, 100) // Green
    } else if level < 0.9 {
        egui::Color32::from_rgb(255, 200, 0) // Yellow
    } else {
        egui::Color32::from_rgb(255, 50, 50) // Red
    };
    
    if level_width > 0.0 {
        painter.rect_filled(
            egui::Rect::from_min_size(
                response.rect.min,
                egui::vec2(level_width, height)
            ),
            4.0,
            color
        );
    }
}

/// A stat card showing a label and value
pub fn stat_card(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(label).size(10.0).weak());
                ui.label(egui::RichText::new(value).size(16.0).strong());
            });
        });
}

/// A modal dialog
pub fn modal_dialog(
    ctx: &egui::Context,
    title: &str,
    content: impl FnOnce(&mut egui::Ui),
    buttons: impl FnOnce(&mut egui::Ui) -> bool,
) -> bool {
    let mut closed = false;
    
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            content(ui);
            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    closed = buttons(ui);
                });
            });
        });
    
    closed
}
