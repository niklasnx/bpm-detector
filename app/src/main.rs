use bpmdetector::gui;

fn main() -> eframe::Result<()> {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([420.0, 340.0])
            .with_resizable(false)
            .with_title("BPM Detector"),
        ..Default::default()
    };

    let (producer, consumer) = rtrb::RingBuffer::new(256);

    eframe::run_native(
        "BPM Detector",
        options,
        Box::new(move |_cc| Ok(Box::new(gui::App::new(producer, consumer)))),
    )
}
