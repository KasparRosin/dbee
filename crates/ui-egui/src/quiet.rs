//! The window's one clock for what waits: nothing shows that a job waits (a timer, a running
//! mark, CANCEL, a dim) before the job has run [`QUIET`], so a fast one never flickers. The
//! window notes the first frame that saw each job, on egui's clock, and every surface asks it
//! ([`Quiet::past`]); a job the engine no longer runs is forgotten once a frame ([`Quiet::keep`]),
//! so the next one starts its own 300 ms and its timer from `00:00.0`.

use std::collections::HashMap;
use std::time::Duration;

use dbee_engine::{JobId, TabId};

/// How long a job runs before anything shows that it waits.
pub(crate) const QUIET: Duration = Duration::from_millis(300);

/// How often a shown timer moves.
pub(crate) const TICK: Duration = Duration::from_millis(100);

/// What the clock's roundings may take off a time.
const SLACK: Duration = Duration::from_micros(1);

/// A job the window watches, by what the engine names it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Watch {
    /// A tab's job, by its marker (`loading`, `counting`, `applying`, `running`).
    Tab(TabId, JobId),
    /// The connection form's test or connect (`ConnScreen.running`'s job).
    Form(JobId),
    /// A job no tab owns, by its command's label: `connection.reconnect`.
    Command(&'static str),
}

/// When the window first saw each job it watches, in seconds on egui's clock.
#[derive(Debug, Default)]
pub(crate) struct Quiet {
    first: HashMap<Watch, f64>,
}

impl Quiet {
    /// `watch` runs now: notes the first frame that saw it (egui's `input.time`), and gives how long
    /// since then once that is [`QUIET`] or more, else `None`. Asks for a frame at the [`QUIET`]
    /// mark, and every [`TICK`] once past it, so the state shows on time and its timer moves.
    pub fn past(&mut self, ctx: &egui::Context, watch: Watch) -> Option<Duration> {
        let now = ctx.input(|i| i.time);
        let first = *self.first.entry(watch).or_insert(now);
        // Egui's clock is an f64 of seconds: 1.4 - 1.1 falls short of 0.3 by a rounding.
        let ran = Duration::try_from_secs_f64(now - first).unwrap_or_default() + SLACK;
        match QUIET.checked_sub(ran).filter(|left| !left.is_zero()) {
            Some(left) => {
                ctx.request_repaint_after(left);
                None
            }
            None => {
                ctx.request_repaint_after(TICK);
                Some(ran)
            }
        }
    }

    /// Forgets every watch that `live` says no longer runs.
    pub fn keep(&mut self, live: impl Fn(&Watch) -> bool) {
        self.first.retain(|watch, _| live(watch));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame at `time` on egui's clock: what `past` gave for `watch`, and the frame's repaint delay.
    fn at(ctx: &egui::Context, quiet: &mut Quiet, time: f64, watch: Watch) -> (Option<Duration>, Duration) {
        let mut got = None;
        let delay = frame(ctx, time, |ui| got = quiet.past(ui.ctx(), watch));
        (got, delay)
    }

    /// A frame at `time` drawing `draw`; its repaint delay.
    fn frame(ctx: &egui::Context, time: f64, draw: impl FnMut(&mut egui::Ui)) -> Duration {
        let mut out = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, draw);
        // The font atlas this first frame made goes nowhere.
        out.textures_delta.clear();
        out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Within a millisecond (egui's clock is an f64 of seconds).
    fn about(got: Option<Duration>, want: Duration) -> bool {
        got.is_some_and(|got| got.abs_diff(want) < ms(1))
    }

    #[test]
    fn nothing_before_300_ms_then_the_time_since_first_sight() {
        let (ctx, mut quiet) = (egui::Context::default(), Quiet::default());
        let watch = Watch::Tab(TabId(1), JobId(7));
        assert_eq!(at(&ctx, &mut quiet, 10.0, watch).0, None);
        assert_eq!(at(&ctx, &mut quiet, 10.29, watch).0, None);
        assert!(about(at(&ctx, &mut quiet, 10.30, watch).0, ms(300)));
        assert!(about(at(&ctx, &mut quiet, 11.2, watch).0, ms(1200)));
    }

    #[test]
    fn a_watch_restarts_per_job_and_keeps_across_tabs() {
        let (ctx, mut quiet) = (egui::Context::default(), Quiet::default());
        let (first, second) = (Watch::Tab(TabId(1), JobId(7)), Watch::Tab(TabId(1), JobId(8)));
        assert_eq!(at(&ctx, &mut quiet, 0.0, first).0, None);
        // Not asked while another tab shows, but still running.
        for tenth in 1..10 {
            quiet.keep(|w| *w == first);
            frame(&ctx, f64::from(tenth) / 10.0, |_| {});
        }
        assert!(about(at(&ctx, &mut quiet, 1.0, first).0, ms(1000)), "kept across the switch");
        quiet.keep(|_| false);
        assert_eq!(at(&ctx, &mut quiet, 1.1, second).0, None, "the next job starts its own 300 ms");
        assert!(about(at(&ctx, &mut quiet, 1.4, second).0, ms(300)));
        quiet.keep(|_| false);
        assert_eq!(at(&ctx, &mut quiet, 1.5, first).0, None, "a forgotten job seen again starts again");
    }

    #[test]
    fn it_asks_for_a_frame_at_the_mark_and_every_tick() {
        let (ctx, mut quiet) = (egui::Context::default(), Quiet::default());
        let watch = Watch::Form(JobId(3));
        let _ = at(&ctx, &mut quiet, 0.0, watch);
        let (shown, delay) = at(&ctx, &mut quiet, 0.1, watch);
        assert_eq!(shown, None);
        assert!(delay <= ms(200) + ms(17), "a frame at the 300 ms mark: {delay:?}");
        let (shown, delay) = at(&ctx, &mut quiet, 0.5, watch);
        assert!(shown.is_some());
        assert!(delay <= TICK, "a frame every tick once shown: {delay:?}");
    }
}
