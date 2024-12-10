use core::{error::Error, f64::consts::PI, num::NonZeroU16, time::Duration};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
use std::{fs, path::Path};

#[cfg(target_arch = "wasm32")]
use instant::Instant;
use serde::{Deserialize, Serialize};

pub trait GeneratorFactory {
    fn create(self) -> Result<Box<dyn Generator>, Box<dyn Error>>;
    fn reduce(&mut self, factor: NonZeroU16);
    fn duration(&self) -> Duration;
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum Config {
    Const(ConstGeneratorConfig),
    Line(LineGeneratorConfig),
    Sin(SinGeneratorConfig),
    Seq(SeqGeneratorConfig),
    Sum(SumGeneratorConfig),
}

impl GeneratorFactory for Config {
    fn create(self) -> Result<Box<dyn Generator>, Box<dyn Error>> {
        match self {
            Config::Const(c) => c.create(),
            Config::Line(c) => c.create(),
            Config::Sin(c) => c.create(),
            Config::Seq(c) => c.create(),
            Config::Sum(c) => c.create(),
        }
    }

    fn reduce(&mut self, factor: NonZeroU16) {
        match self {
            Config::Const(c) => c.reduce(factor),
            Config::Line(c) => c.reduce(factor),
            Config::Sin(c) => c.reduce(factor),
            Config::Seq(c) => c.reduce(factor),
            Config::Sum(c) => c.reduce(factor),
        }
    }

    fn duration(&self) -> Duration {
        match self {
            Config::Const(c) => c.duration(),
            Config::Line(c) => c.duration(),
            Config::Sin(c) => c.duration(),
            Config::Seq(c) => c.duration(),
            Config::Sum(c) => c.duration(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct ConstGeneratorConfig {
    rps: u64,
    /// Duration in seconds.
    duration: u64,
}

impl GeneratorFactory for ConstGeneratorConfig {
    fn create(self) -> Result<Box<dyn Generator>, Box<dyn Error>> {
        let g = LineGenerator::new(self.rps, self.rps, Duration::from_secs(self.duration));

        Ok(Box::new(g))
    }

    fn reduce(&mut self, factor: NonZeroU16) {
        self.rps /= factor.get() as u64;
    }

    fn duration(&self) -> Duration {
        Duration::from_secs(self.duration)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct LineGeneratorConfig {
    rps: (u64, u64),
    /// Duration in seconds.
    duration: u64,
}

impl GeneratorFactory for LineGeneratorConfig {
    fn create(self) -> Result<Box<dyn Generator>, Box<dyn Error>> {
        let (rps0, rps1) = self.rps;
        let g = LineGenerator::new(rps0, rps1, Duration::from_secs(self.duration));

        Ok(Box::new(g))
    }

    fn reduce(&mut self, factor: NonZeroU16) {
        let factor = factor.get() as u64;
        self.rps.0 /= factor;
        self.rps.1 /= factor;
    }

    fn duration(&self) -> Duration {
        Duration::from_secs(self.duration)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct SinGeneratorConfig {
    pps: (u64, u64),
    pps_min: u64,
    pps_max: u64,
    approximate_period: u64,
    /// Duration in seconds.
    duration: u64,
}

impl SinGeneratorConfig {
    fn find_closest_phase(&self, pps_mid: f64, a: f64) -> Result<(f64, f64), Box<dyn Error>> {
        let mut prev_err = f64::INFINITY;
        let mut prev_phase = 0.0;
        let mut prev_k = 0.0;

        for n in -100..100 {
            let n = n as f64;
            let phase = f64::asin((self.pps.0 as f64 - pps_mid) / a) + 2.0 * PI * n;
            let k = (f64::asin((self.pps.1 as f64 - pps_mid) / a) - phase)
                / Duration::from_secs(self.duration).as_secs_f64();
            let period = f64::abs(2.0 * PI / k);
            let err = f64::abs(period - Duration::from_secs(self.approximate_period).as_secs_f64());
            if err > prev_err {
                return Ok((prev_phase, prev_k));
            }

            prev_phase = phase;
            prev_k = k;
            prev_err = err;
        }

        Err("failed to find parameters for sin function".into())
    }
}

impl GeneratorFactory for SinGeneratorConfig {
    fn create(self) -> Result<Box<dyn Generator>, Box<dyn Error>> {
        let a = (self.pps_max as f64 - self.pps_min as f64) / 2.0;
        let pps_mid = self.pps_min as f64 + a;
        let (phase, k) = self.find_closest_phase(pps_mid, a)?;
        let g = SinGenerator::new(pps_mid, a, k, phase, Duration::from_secs(self.duration));

        Ok(Box::new(g))
    }

    fn reduce(&mut self, factor: NonZeroU16) {
        let factor = factor.get() as u64;
        self.pps.0 /= factor;
        self.pps.1 /= factor;
        self.pps_min /= factor;
        self.pps_max /= factor;
    }

    fn duration(&self) -> Duration {
        Duration::from_secs(self.duration)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SeqGeneratorConfig {
    generators: Vec<Config>,
}

impl GeneratorFactory for SeqGeneratorConfig {
    fn create(self) -> Result<Box<dyn Generator>, Box<dyn Error>> {
        let generators: Result<Vec<Box<dyn Generator>>, Box<dyn Error>> =
            self.generators.into_iter().map(|v| v.create()).collect();
        let g = SeqGenerator::new(generators?);

        Ok(Box::new(g))
    }

    fn reduce(&mut self, factor: NonZeroU16) {
        for gen in &mut self.generators {
            gen.reduce(factor);
        }
    }

    fn duration(&self) -> Duration {
        let mut out = Duration::from_secs(0);
        for gen in &self.generators {
            out += gen.duration();
        }

        out
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SumGeneratorConfig {
    generators: Vec<Config>,
}

impl GeneratorFactory for SumGeneratorConfig {
    fn create(self) -> Result<Box<dyn Generator>, Box<dyn Error>> {
        let generators: Result<Vec<Box<dyn Generator>>, Box<dyn Error>> =
            self.generators.into_iter().map(|v| v.create()).collect();
        let g = SumGenerator::new(generators?);

        Ok(Box::new(g))
    }

    fn reduce(&mut self, factor: NonZeroU16) {
        for gen in &mut self.generators {
            gen.reduce(factor);
        }
    }

    fn duration(&self) -> Duration {
        let mut out = Duration::from_secs(0);

        for gen in &self.generators {
            out = core::cmp::max(out, gen.duration());
        }

        out
    }
}

pub trait Generator {
    fn activate(&mut self);
    fn activate_at(&mut self, now: Instant);
    fn current(&mut self) -> Option<u64>;
    fn current_at(&mut self, now: Instant) -> Option<u64>;
}

impl<T> Generator for Box<T>
where
    T: Generator + ?Sized,
{
    fn activate(&mut self) {
        (**self).activate()
    }

    fn activate_at(&mut self, now: Instant) {
        (**self).activate_at(now)
    }

    fn current(&mut self) -> Option<u64> {
        (**self).current()
    }

    fn current_at(&mut self, now: Instant) -> Option<u64> {
        (**self).current_at(now)
    }
}

pub struct SuspendableGenerator<G> {
    g: G,
    suspended_at: Option<Instant>,
    suspended_duration: Duration,
    /// Manually set generator value.
    value: Option<u64>,
}

impl<G> SuspendableGenerator<G>
where
    G: Generator,
{
    #[inline]
    pub const fn new(g: G) -> Self {
        Self {
            g,
            suspended_at: None,
            suspended_duration: Duration::new(0, 0),
            value: None,
        }
    }

    pub fn suspend(&mut self) {
        if self.suspended_at.is_some() {
            return;
        }

        self.value = None;
        self.suspended_at = Some(Instant::now());
    }

    pub fn resume(&mut self) {
        if let Some(prev) = self.suspended_at.take() {
            self.value = None;
            self.suspended_duration += Instant::now().duration_since(prev);
        }
    }

    pub fn set<T>(&mut self, value: T)
    where
        T: Into<Option<u64>>,
    {
        let value = value.into();

        if value.is_some() {
            self.suspend();
        } else {
            self.resume();
        }

        self.value = value;
    }
}

impl<G> Generator for SuspendableGenerator<G>
where
    G: Generator,
{
    #[inline]
    fn activate(&mut self) {
        self.g.activate();
    }

    #[inline]
    fn activate_at(&mut self, now: Instant) {
        self.g.activate_at(now);
    }

    #[inline]
    fn current(&mut self) -> Option<u64> {
        self.current_at(Instant::now())
    }

    #[inline]
    fn current_at(&mut self, now: Instant) -> Option<u64> {
        match self.suspended_at {
            Some(..) => self.value.or(Some(0)),
            None => self.g.current_at(now - self.suspended_duration),
        }
    }
}

/// Line generator.
///
/// f(t) = k * (t - t0) + b
pub struct LineGenerator {
    k: i64,
    b: u64,
    t0: Instant,
    duration: Duration,
}

impl LineGenerator {
    /// Parameters `rps0` and `rps1` shows PPS at the beginning and at the end
    /// of generation.
    ///
    /// Both of them must be 63-bit(!) unsigned integers.
    /// Maximum allowed diff between them is 1Grps (1'000'000'000 pps).
    pub fn new(rps0: u64, rps1: u64, duration: Duration) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let k = (rps1 as i64 - rps0 as i64) * 1000000000 / duration.as_nanos() as i64;
        #[cfg(target_arch = "wasm32")]
        let k = (rps1 as i64 - rps0 as i64) / duration.as_secs_f64() as i64;

        let b = rps0;

        Self { k, b, t0: Instant::now(), duration }
    }
}

impl Generator for LineGenerator {
    fn activate(&mut self) {
        self.activate_at(Instant::now());
    }

    fn activate_at(&mut self, now: Instant) {
        self.t0 = now;
    }

    fn current(&mut self) -> Option<u64> {
        self.current_at(Instant::now())
    }

    fn current_at(&mut self, now: Instant) -> Option<u64> {
        let elapsed = now.duration_since(self.t0);

        if elapsed > self.duration {
            None
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            let v = ((self.k * elapsed.as_nanos() as i64) / 1000000000 + self.b as i64) as u64;
            #[cfg(target_arch = "wasm32")]
            let v = ((self.k * elapsed.as_secs_f64() as i64) + self.b as i64) as u64;

            Some(v)
        }
    }
}

/// f(t) = pps_mid + a * sin(k * (t-t0) + phase)
pub struct SinGenerator {
    pps_mid: f64,
    a: f64,
    k: f64,
    phase: f64,
    t0: Instant,
    duration: Duration,
}

impl SinGenerator {
    pub fn new(pps_mid: f64, a: f64, k: f64, phase: f64, duration: Duration) -> Self {
        Self {
            pps_mid,
            a,
            k,
            phase,
            t0: Instant::now(),
            duration,
        }
    }
}

impl Generator for SinGenerator {
    fn activate(&mut self) {
        self.t0 = Instant::now();
    }

    fn activate_at(&mut self, now: Instant) {
        self.t0 = now;
    }

    fn current(&mut self) -> Option<u64> {
        self.current_at(Instant::now())
    }

    fn current_at(&mut self, now: Instant) -> Option<u64> {
        let elapsed = now.duration_since(self.t0);

        if elapsed > self.duration {
            None
        } else {
            let v = core::cmp::max(
                0,
                (self.pps_mid + self.a * f64::sin(self.k * elapsed.as_secs_f64() + self.phase)) as u64,
            );
            Some(v)
        }
    }
}

pub struct SeqGenerator {
    idx: usize,
    generators: Vec<Box<dyn Generator>>,
}

impl SeqGenerator {
    pub fn new(generators: Vec<Box<dyn Generator>>) -> Self {
        Self { idx: 0, generators }
    }
}

impl Generator for SeqGenerator {
    fn activate(&mut self) {
        if self.idx >= self.generators.len() {
            return;
        }
        self.generators[self.idx].activate();
    }

    fn activate_at(&mut self, now: Instant) {
        if self.idx >= self.generators.len() {
            return;
        }
        self.generators[self.idx].activate_at(now);
    }

    fn current(&mut self) -> Option<u64> {
        if self.idx >= self.generators.len() {
            return None;
        }

        match self.generators[self.idx].current() {
            Some(v) => Some(v),
            None => {
                self.idx += 1;
                self.activate();
                self.current()
            }
        }
    }

    fn current_at(&mut self, now: Instant) -> Option<u64> {
        if self.idx >= self.generators.len() {
            return None;
        }

        match self.generators[self.idx].current_at(now) {
            Some(v) => Some(v),
            None => {
                self.idx += 1;
                self.activate_at(now);
                self.current_at(now)
            }
        }
    }
}

pub struct SumGenerator {
    generators: Vec<Box<dyn Generator>>,
}

impl SumGenerator {
    pub fn new(generators: Vec<Box<dyn Generator>>) -> Self {
        Self { generators }
    }
}

impl Generator for SumGenerator {
    fn activate(&mut self) {
        for g in &mut self.generators {
            g.activate();
        }
    }

    fn activate_at(&mut self, now: Instant) {
        for g in &mut self.generators {
            g.activate_at(now);
        }
    }

    fn current(&mut self) -> Option<u64> {
        let mut out = None;
        for g in &mut self.generators {
            match (out, g.current()) {
                (.., None) => {}
                (None, Some(v)) => out = Some(v),
                (Some(o), Some(v)) => out = Some(o + v),
            }
        }

        out
    }

    fn current_at(&mut self, now: Instant) -> Option<u64> {
        let mut out = None;
        for g in &mut self.generators {
            match (out, g.current_at(now)) {
                (.., None) => {}
                (None, Some(v)) => out = Some(v),
                (Some(o), Some(v)) => out = Some(o + v),
            }
        }

        out
    }
}

pub fn load<P: AsRef<Path>>(path: P) -> Result<Box<dyn Generator>, Box<dyn Error>> {
    let cfg = load_config(path)?;
    let gen = cfg.create()?;

    Ok(gen)
}

pub fn load_config<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn Error>> {
    let data = fs::read(path)?;
    let cfg: Config = serde_yaml::from_slice(&data)?;

    Ok(cfg)
}
