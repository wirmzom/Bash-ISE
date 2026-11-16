use eframe::egui;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::thread;

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Bash-ISE",
        native_options,
        Box::new(|cc| Ok(Box::new(BashISE::new(cc)))),
    )
}

#[derive(Default)]
struct BashISE {
    script: String,              // editor content
    output: String,              // output pane content
    rx: Option<Receiver<String>>, // Some(..) while a script is running
}

impl BashISE {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    /// Run the current script on a background thread.
    fn run_script(&mut self, ctx: egui::Context) {
        let script = self.script.clone(); // the thread needs its own copy (ownership!)
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.output.clear();

        thread::spawn(move || {
            let result = match Command::new("bash").arg("-c").arg(&script).output() {
                Ok(o) => format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => format!("Failed to start bash: {e}"),
            };
            let _ = tx.send(result);
            ctx.request_repaint(); // wake the UI up
        });
    }
}

impl eframe::App for BashISE {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 1. Check whether the background thread has finished
        if let Some(rx) = &self.rx {
            if let Ok(result) = rx.try_recv() {
                self.output = result;
                self.rx = None;
            }
        }

        let running = self.rx.is_some();
        let f5 = ui.input(|i| i.key_pressed(egui::Key::F5));
        let ctx = ui.ctx().clone();

        egui::CentralPanel::default().show(ui, |ui| {
            // 2. Toolbar
            ui.horizontal(|ui| {
                let clicked = ui
                    .add_enabled(!running, egui::Button::new("▶ Run (F5)"))
                    .clicked();
                if (clicked || f5) && !running {
                    self.run_script(ctx.clone());
                }
                if running {
                    ui.spinner();
                }
            });
            ui.separator();

            // 3. Editor (top ~60% of the space)
            let h = ui.available_height();
            egui::ScrollArea::vertical()
                .id_salt("editor")
                .max_height(h * 0.6)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.script)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(15),
                    );
                });

            ui.separator();

            // 4. Output pane
            egui::ScrollArea::vertical()
                .id_salt("output")
                .show(ui, |ui| {
                    ui.monospace(&self.output);
                });
        });
    }
}