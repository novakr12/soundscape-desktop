use std::time::Instant;

pub const SETTLE_MS: f64 = 48.0;
const LONGEST_FRAME_MS: f64 = 64.0;
const CLOSE_ENOUGH: f64 = 0.5;

pub fn step(remaining: (f64, f64), elapsed_ms: f64) -> (f64, f64) {
    let share = 1.0 - (-elapsed_ms.clamp(0.0, LONGEST_FRAME_MS) / SETTLE_MS).exp();
    let moved = (remaining.0 * share, remaining.1 * share);
    let left = (remaining.0 - moved.0, remaining.1 - moved.1);
    if left.0.abs() < CLOSE_ENOUGH && left.1.abs() < CLOSE_ENOUGH {
        remaining
    } else {
        moved
    }
}

#[derive(Default)]
pub struct Glide {
    remaining: (f64, f64),
    last: Option<Instant>,
    point: (f64, f64),
}

impl Glide {
    pub fn add(&mut self, x: f64, y: f64, point: (f64, f64), now: Instant) {
        let reversed = self.remaining.0 * x < 0.0 || self.remaining.1 * y < 0.0;
        if reversed {
            self.remaining = (0.0, 0.0);
        }
        if !self.active() {
            self.last = Some(now);
        }
        self.remaining.0 += x;
        self.remaining.1 += y;
        self.point = point;
    }

    pub fn active(&self) -> bool {
        self.remaining.0 != 0.0 || self.remaining.1 != 0.0
    }

    pub fn point(&self) -> (f64, f64) {
        self.point
    }

    pub fn tick(&mut self, now: Instant) -> Option<(f64, f64)> {
        if !self.active() {
            self.last = None;
            return None;
        }
        let elapsed = self
            .last
            .map(|last| now.duration_since(last).as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        self.last = Some(now);
        let moved = step(self.remaining, elapsed);
        self.remaining.0 -= moved.0;
        self.remaining.1 -= moved.1;
        if self.remaining.0.abs() < 1e-9 && self.remaining.1.abs() < 1e-9 {
            self.remaining = (0.0, 0.0);
        }
        (moved.0 != 0.0 || moved.1 != 0.0).then_some(moved)
    }
}

pub const FRICTION_MS: f64 = 325.0;
const SAMPLE_MS: f64 = 100.0;
const SHORTEST_SAMPLE_MS: f64 = 16.0;
const SLOWEST: f64 = 0.03;
const FASTEST: f64 = 8.0;

#[derive(Default)]
pub struct Fling {
    samples: Vec<(Instant, (f64, f64))>,
    velocity: (f64, f64),
    last: Option<Instant>,
    point: (f64, f64),
}

impl Fling {
    pub fn touch(&mut self, x: f64, y: f64, point: (f64, f64), now: Instant) {
        self.velocity = (0.0, 0.0);
        self.last = None;
        self.point = point;
        self.samples
            .retain(|(at, _)| ms_between(*at, now) <= SAMPLE_MS);
        self.samples.push((now, (x, y)));
    }

    pub fn release(&mut self, now: Instant) {
        let recent: Vec<_> = self
            .samples
            .drain(..)
            .filter(|(at, _)| ms_between(*at, now) <= SAMPLE_MS)
            .collect();
        let Some((first, _)) = recent.first() else {
            return;
        };
        let span = ms_between(*first, now).max(SHORTEST_SAMPLE_MS);
        let total = recent.iter().fold((0.0, 0.0), |sum, (_, moved)| {
            (sum.0 + moved.0, sum.1 + moved.1)
        });
        let velocity = (
            (total.0 / span).clamp(-FASTEST, FASTEST),
            (total.1 / span).clamp(-FASTEST, FASTEST),
        );
        if speed(velocity) >= SLOWEST {
            self.velocity = velocity;
            self.last = Some(now);
        }
    }

    pub fn stop(&mut self) {
        self.samples.clear();
        self.velocity = (0.0, 0.0);
        self.last = None;
    }

    pub fn active(&self) -> bool {
        self.last.is_some()
    }

    pub fn point(&self) -> (f64, f64) {
        self.point
    }

    pub fn tick(&mut self, now: Instant) -> Option<(f64, f64)> {
        let last = self.last?;
        let elapsed = ms_between(last, now).min(LONGEST_FRAME_MS);
        self.last = Some(now);
        let kept = (-elapsed / FRICTION_MS).exp();
        let moved = (
            self.velocity.0 * FRICTION_MS * (1.0 - kept),
            self.velocity.1 * FRICTION_MS * (1.0 - kept),
        );
        self.velocity = (self.velocity.0 * kept, self.velocity.1 * kept);
        if speed(self.velocity) < SLOWEST {
            self.stop();
        }
        (moved.0 != 0.0 || moved.1 != 0.0).then_some(moved)
    }
}

fn ms_between(earlier: Instant, later: Instant) -> f64 {
    later.saturating_duration_since(earlier).as_secs_f64() * 1000.0
}

fn speed(velocity: (f64, f64)) -> f64 {
    velocity.0.hypot(velocity.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn coasts_after_a_quick_swipe_and_slows_down() {
        let start = Instant::now();
        let mut fling = Fling::default();
        for step in 0..5 {
            fling.touch(
                0.0,
                40.0,
                (1.0, 2.0),
                start + Duration::from_millis(step * 16),
            );
        }
        fling.release(start + Duration::from_millis(80));
        assert!(fling.active());
        let mut now = start + Duration::from_millis(80);
        let mut steps = Vec::new();
        while fling.active() {
            now += Duration::from_millis(16);
            if let Some((_, y)) = fling.tick(now) {
                steps.push(y);
            }
            assert!(steps.len() < 400);
        }
        assert!(steps[0] > steps[steps.len() - 1]);
        assert!(steps.iter().all(|step| *step > 0.0));
        let travelled: f64 = steps.iter().sum();
        assert!(travelled > 300.0 && travelled < 1000.0, "{travelled}");
        assert_eq!(fling.point(), (1.0, 2.0));
    }

    #[test]
    fn stays_put_after_a_slow_drag_or_a_pause() {
        let start = Instant::now();
        let mut slow = Fling::default();
        slow.touch(0.0, 1.0, (0.0, 0.0), start);
        slow.release(start + Duration::from_millis(90));
        assert!(!slow.active());
        let mut paused = Fling::default();
        paused.touch(0.0, 50.0, (0.0, 0.0), start);
        paused.release(start + Duration::from_millis(300));
        assert!(!paused.active());
        let mut stopped = Fling::default();
        stopped.touch(0.0, 50.0, (0.0, 0.0), start);
        stopped.release(start + Duration::from_millis(20));
        assert!(stopped.active());
        stopped.stop();
        assert!(!stopped.active());
        assert_eq!(stopped.tick(start + Duration::from_millis(40)), None);
    }

    #[test]
    fn eases_out_quickly() {
        let (_, first) = step((0.0, 100.0), 16.0);
        assert!(first > 25.0 && first < 35.0, "{first}");
        let (_, later) = step((0.0, 10.0), 16.0);
        assert!(later < first);
        assert_eq!(step((0.0, 0.4), 16.0), (0.0, 0.4));
        let (_, capped) = step((0.0, 100.0), 1000.0);
        assert!(capped < 80.0, "{capped}");
    }

    #[test]
    fn travels_the_whole_distance() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.add(0.0, 152.0, (5.0, 6.0), start);
        let mut travelled = 0.0;
        let mut frames = 0;
        let mut now = start;
        while glide.active() {
            now += Duration::from_millis(8);
            if let Some((_, y)) = glide.tick(now) {
                travelled += y;
            }
            frames += 1;
            assert!(frames < 200);
        }
        assert!((travelled - 152.0).abs() < 1e-6, "{travelled}");
        assert!(frames < 60, "{frames}");
        assert_eq!(glide.point(), (5.0, 6.0));
    }

    #[test]
    fn adds_notches_and_turns_around() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.add(0.0, 76.0, (0.0, 0.0), start);
        glide.add(0.0, 76.0, (0.0, 0.0), start);
        let moved = glide.tick(start + Duration::from_millis(16)).unwrap();
        assert!(moved.1 > 40.0, "{}", moved.1);
        glide.add(0.0, -76.0, (0.0, 0.0), start);
        let back = glide.tick(start + Duration::from_millis(32)).unwrap();
        assert!(back.1 < 0.0);
    }
}
