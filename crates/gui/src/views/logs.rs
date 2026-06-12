use eframe::egui;

pub fn show(ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.heading(
            egui::RichText::new("Service Logs")
                .color(egui::Color32::from_rgb(255, 255, 255))
                .size(24.0)
                .strong(),
        );
        ui.label(
            egui::RichText::new("Inspect recent output logs from the background daemon.")
                .color(egui::Color32::from_rgb(140, 155, 175))
                .size(13.0),
        );
        ui.add_space(20.0);

        // Resolve log path
        let exe_dir = match std::env::current_exe() {
            Ok(p) => p.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            Err(_) => std::path::PathBuf::default(),
        };
        let log_path = exe_dir.join("backup-agent.log");

        if log_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&log_path) {
                // Get the last 150 lines of logs
                let lines: Vec<&str> = content.lines().collect();
                let start = if lines.len() > 150 { lines.len() - 150 } else { 0 };

                ui.horizontal(|ui| {
                    let clear_btn = egui::Button::new(
                        egui::RichText::new("🗑 Clear Log File")
                            .color(egui::Color32::from_rgb(255, 255, 255))
                            .strong()
                    )
                    .fill(egui::Color32::from_rgb(185, 28, 28))
                    .min_size(egui::vec2(120.0, 28.0));

                    let refresh_btn = egui::Button::new(
                        egui::RichText::new("🔄 Refresh Logs")
                            .color(egui::Color32::from_rgb(255, 255, 255))
                            .strong()
                    )
                    .fill(egui::Color32::from_rgb(30, 41, 59))
                    .min_size(egui::vec2(120.0, 28.0));

                    if ui.add(clear_btn).clicked() {
                        let _ = std::fs::write(&log_path, "");
                    }
                    ui.add_space(8.0);
                    if ui.add(refresh_btn).clicked() {
                        ui.ctx().request_repaint();
                    }
                });
                ui.add_space(10.0);

                // Use ScrollArea::both() to scroll horizontally and vertically
                egui::ScrollArea::both()
                    .max_width(ui.available_width() - 24.0)
                    .max_height(ui.available_height() - 80.0)
                    .stick_to_bottom(true) // Auto-scroll to latest logs
                    .show(ui, |ui| {
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(15, 23, 42)) // dark slate/black terminal background
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(51, 65, 85))) // slate border
                            .rounding(6.0)
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                // Set minimum width so horizontal scroll kicks in
                                ui.set_min_width(ui.available_width());
                                ui.vertical(|ui| {
                                    for line in lines[start..].iter() {
                                        let mut color = egui::Color32::from_rgb(203, 213, 225); // default light grey
                                        if line.contains("ERROR") || line.contains("FATAL") {
                                            color = egui::Color32::from_rgb(248, 113, 113); // soft light red
                                        } else if line.contains("WARN") || line.contains("WARNING") {
                                            color = egui::Color32::from_rgb(251, 146, 60); // soft light orange
                                        } else if line.contains("INFO") {
                                            color = egui::Color32::from_rgb(94, 234, 212); // soft light teal
                                        } else if line.contains("DEBUG") {
                                            color = egui::Color32::from_rgb(148, 163, 184); // dark grey/slate
                                        }

                                        // We use Label with wrap(false) to ensure horizontal scrolling works without wrapping
                                        let text = egui::RichText::new(*line)
                                            .monospace()
                                            .color(color)
                                            .size(11.0);
                                        
                                        ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
                                    }
                                });
                            });
                    });
            } else {
                ui.label("Error reading log file contents.");
            }
        } else {
            ui.group(|ui| {
                ui.set_width(ui.available_width() - 24.0);
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new("No service log file found yet.")
                            .color(egui::Color32::from_rgb(110, 125, 150))
                            .italics()
                    );
                    ui.label(
                        egui::RichText::new(format!("Ensure the service is running and creating logs at:\n{}", log_path.display()))
                            .color(egui::Color32::from_rgb(110, 125, 150))
                            .size(11.0)
                    );
                    ui.add_space(20.0);
                });
            });
        }
    });
}
