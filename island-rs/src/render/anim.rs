//! Spring and tweens. Nothing here runs on its own: the window loop asks
//! `active()` and only produces frames while something still moves.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::Instant;

#[derive(Clone, Copy)]
pub enum Ease {
    Linear,
    CubicOut,
    SineInOut,
    /// Overshoots, then settles. The value is the overshoot amplitude.
    BackOut(f32),
}

impl Ease {
    fn apply(self, t: f32) -> f32 {
        match self {
            Ease::Linear => t,
            Ease::CubicOut => {
                let u = 1.0 - t;
                1.0 - u * u * u
            }
            Ease::SineInOut => 0.5 * (1.0 - (std::f32::consts::PI * t).cos()),
            Ease::BackOut(a) => {
                let u = 1.0 - t;
                1.0 - (u * u * u - u * a * (std::f32::consts::PI * u).sin())
            }
        }
    }
}

struct Tween {
    from: f32,
    to: f32,
    start: f64,
    dur: f64,
    ease: Ease,
}

impl Tween {
    fn value(&self, now: f64) -> f32 {
        if now <= self.start {
            self.from
        } else if now >= self.start + self.dur {
            self.to
        } else {
            let t = ((now - self.start) / self.dur) as f32;
            self.from + (self.to - self.from) * self.ease.apply(t)
        }
    }
}

pub struct Anims<K> {
    map: HashMap<K, Tween>,
    clock: Instant,
    /// Time of the frame being drawn.
    now: f64,
}

impl<K: Hash + Eq + Copy> Anims<K> {
    pub fn new() -> Self {
        Anims { map: HashMap::new(), clock: Instant::now(), now: 0.0 }
    }

    pub fn time(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// Call at the start of each frame.
    pub fn tick(&mut self) {
        self.now = self.time();
    }

    pub fn get(&self, key: K, default: f32) -> f32 {
        match self.map.get(&key) {
            Some(t) => t.value(self.now),
            None => default,
        }
    }

    /// Animates from the current value (or `default`) to `target`.
    pub fn to(&mut self, key: K, default: f32, target: f32, ms: u32, ease: Ease) {
        self.to_delayed(key, default, target, 0, ms, ease);
    }

    /// Same as `to`, but holds the current value for `delay_ms` first.
    pub fn to_delayed(&mut self, key: K, default: f32, target: f32, delay_ms: u32, ms: u32, ease: Ease) {
        let now = self.time();
        let from = match self.map.get(&key) {
            Some(t) => t.value(now),
            None => default,
        };
        self.run(key, from, target, delay_ms, ms, ease);
    }

    /// Animates from an explicit start value.
    pub fn run(&mut self, key: K, from: f32, to: f32, delay_ms: u32, ms: u32, ease: Ease) {
        let now = self.time();
        self.map.insert(
            key,
            Tween { from, to, start: now + delay_ms as f64 / 1000.0, dur: ms as f64 / 1000.0, ease },
        );
    }

    pub fn remove(&mut self, key: K) {
        self.map.remove(&key);
    }

    pub fn active(&self) -> bool {
        let now = self.time();
        self.map.values().any(|t| now < t.start + t.dur)
    }
}

/// Damped spring. Keeps its velocity when the target changes.
#[derive(Clone, Copy)]
pub struct Spring {
    pub x: f64,
    pub v: f64,
    pub target: f64,
}

impl Spring {
    pub const STIFFNESS: f64 = 260.0;
    pub const DAMPING: f64 = 24.0;

    pub fn new(x: f64) -> Self {
        Spring { x, v: 0.0, target: x }
    }

    pub fn step(&mut self, dt: f64) {
        let a = -Self::STIFFNESS * (self.x - self.target) - Self::DAMPING * self.v;
        self.v += a * dt;
        self.x += self.v * dt;
    }

    pub fn settled(&self) -> bool {
        (self.x - self.target).abs() < 0.3 && self.v.abs() < 3.0
    }

    pub fn snap(&mut self) {
        self.x = self.target;
        self.v = 0.0;
    }
}
