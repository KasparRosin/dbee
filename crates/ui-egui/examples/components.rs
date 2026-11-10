//! Every control of the design's Components page in every state, in a window and with no
//! database, to look at against the page:
//!
//! `cargo run -p dbee-ui-egui --example components [-- --dark] [--screenshot <file.png>]`
//!
//! `--dark` draws in dark. `--screenshot` saves one picture of the whole showcase once the fonts
//! are in, then quits: the window scrolls through it and the pieces are put together, so a
//! showcase taller than the window comes out whole.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use dbee_ui_egui::components::showcase;
use dbee_ui_egui::theme;
use egui::scroll_area::{ScrollAreaOutput, ScrollBarVisibility};
use egui::{Event, Frame, Rect, ScrollArea, UserData, ViewportCommand};

/// Frames drawn before the first picture: the fonts are in from the first, the window's size
/// has settled by the third.
const SETTLE: u32 = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = if args.iter().any(|a| a == "--dark") { egui::Theme::Dark } else { egui::Theme::Light };
    let file = match args.iter().position(|a| a == "--screenshot") {
        Some(i) => match args.get(i + 1) {
            Some(f) => Some(PathBuf::from(f)),
            None => {
                eprintln!("--screenshot needs the file to save the picture in, for example --screenshot target/components/light.png");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let failed = Arc::new(Mutex::new(None::<String>));
    let app = Showcase { frames: 0, shot: file.map(|file| Shot { file, canvas: None, offset: 0.0, asked: false }), failed: Arc::clone(&failed) };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("dbee components").with_inner_size([1248.0, 900.0]),
        ..eframe::NativeOptions::default()
    };
    let run = eframe::run_native(
        "dbee components",
        options,
        Box::new(move |cc| {
            showcase::prepare(&cc.egui_ctx, mode);
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = run {
        eprintln!("The window could not open: {e}");
        return ExitCode::FAILURE;
    }
    match failed.lock().ok().and_then(|mut f| f.take()) {
        Some(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    }
}

struct Showcase {
    frames: u32,
    shot: Option<Shot>,
    failed: Arc<Mutex<Option<String>>>,
}

/// The picture being taken: the showcase scrolled to `offset`, its window's picture asked for
/// once drawn there, and each piece pasted into `canvas` where it belongs.
struct Shot {
    file: PathBuf,
    canvas: Option<image::RgbaImage>,
    offset: f32,
    asked: bool,
}

impl eframe::App for Showcase {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frames = self.frames.saturating_add(1);
        let ctx = ui.ctx().clone();
        let ground = theme::tokens(&ctx).bg;
        let mut area = ScrollArea::vertical().auto_shrink(false);
        if let Some(shot) = &self.shot {
            area = area.vertical_scroll_offset(shot.offset).scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden);
        }
        let out = egui::CentralPanel::default().frame(Frame::new().fill(ground)).show(ui, |ui| area.show(ui, showcase::show)).inner;
        let Some(shot) = &mut self.shot else { return };
        ctx.request_repaint();
        if self.frames < SETTLE {
            return;
        }
        if let Some(result) = shot.step(&ctx, &out) {
            match result {
                Ok(()) => println!("Saved {}", shot.file.display()),
                Err(why) => {
                    if let Ok(mut failed) = self.failed.lock() {
                        *failed = Some(why);
                    }
                }
            }
            self.shot = None;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

impl Shot {
    /// Asks for the picture of what is drawn now, or pastes it once it came and scrolls on.
    /// `Some` when the whole showcase is in the picture, with how saving it went.
    fn step(&mut self, ctx: &egui::Context, out: &ScrollAreaOutput<()>) -> Option<Result<(), String>> {
        let picture = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });
        let Some(picture) = picture else {
            if !self.asked {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
                self.asked = true;
            }
            return None;
        };
        self.asked = false;
        let ppp = ctx.pixels_per_point();
        let view = out.inner_rect;
        let drawn = out.state.offset.y;
        let canvas = self.canvas.get_or_insert_with(|| image::RgbaImage::new(pixels(view.width(), ppp), pixels(out.content_size.y, ppp)));
        paste(canvas, &picture, view, drawn, ppp);
        let last = (out.content_size.y - view.height()).max(0.0);
        if drawn >= last - 0.5 {
            return Some(save(canvas, &self.file));
        }
        self.offset = (drawn + view.height()).min(last);
        None
    }
}

fn pixels(points: f32, ppp: f32) -> u32 {
    (points * ppp).round().clamp(0.0, u32::MAX as f32) as u32
}

/// Copies the part of `picture` inside `view` into `canvas`, at the height `offset` scrolled to.
fn paste(canvas: &mut image::RgbaImage, picture: &egui::ColorImage, view: Rect, offset: f32, ppp: f32) {
    let [width, height] = picture.size.map(|n| u32::try_from(n).unwrap_or(u32::MAX));
    let (left, top) = (pixels(view.left(), ppp), pixels(view.top(), ppp));
    let (right, bottom) = (pixels(view.right(), ppp).min(width), pixels(view.bottom(), ppp).min(height));
    let down = pixels(offset, ppp);
    for y in top..bottom {
        for x in left..right {
            let at = (y as usize).saturating_mul(width as usize).saturating_add(x as usize);
            let (Some(c), Some(to)) = (picture.pixels.get(at), canvas.get_pixel_mut_checked(x - left, (y - top).saturating_add(down))) else {
                continue;
            };
            *to = image::Rgba(c.to_srgba_unmultiplied());
        }
    }
}

fn save(canvas: &image::RgbaImage, file: &Path) -> Result<(), String> {
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("The folder {} could not be made ({e}): check that you can write there.", dir.display()))?;
    }
    canvas
        .save(file)
        .map_err(|e| format!("The picture could not be saved as {} ({e}): check the folder and that the name ends in .png.", file.display()))
}
