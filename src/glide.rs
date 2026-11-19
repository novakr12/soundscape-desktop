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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
