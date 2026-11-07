//! A streaming windowed-sinc resampler. Each output sample's position comes from its index, so it can't drift over hours, and only a window of input is held.

const PHASES: usize = 512;

pub struct Resampler {
    ratio: f64,
    half: i64,
    width: usize,
    table: Vec<f32>,
    /// Input samples from `base` on.
    held: Vec<f32>,
    base: i64,
    next_out: u64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let ratio = f64::from(from) / f64::from(to);
        let cutoff = (1.0 / ratio).min(1.0) * 0.95;
        let half = (16.0 / cutoff).ceil() as i64;
        let width = (half * 2 + 1) as usize;
        let mut table = vec![0f32; PHASES * width];
        if from != to {
            for p in 0..PHASES {
                let frac = p as f64 / PHASES as f64;
                for k in 0..width {
                    let t = k as f64 - half as f64 - frac;
                    let u = t / (half as f64 + 1.0);
                    let window = 0.42 + 0.5 * (std::f64::consts::PI * u).cos() + 0.08 * (2.0 * std::f64::consts::PI * u).cos();
                    let a = std::f64::consts::PI * t * cutoff;
                    let s = if a.abs() < 1e-9 { 1.0 } else { a.sin() / a };
                    table[p * width + k] = (cutoff * s * window) as f32;
                }
            }
        }
        Self {
            ratio,
            half,
            width,
            table,
            held: vec![],
            base: 0,
            next_out: 0,
        }
    }

    fn sample(&self, at: f64, total: i64, out: &mut Vec<f32>) {
        let centre = at.floor() as i64;
        let phase = ((at - centre as f64) * PHASES as f64) as usize;
        let row = &self.table[phase * self.width..(phase + 1) * self.width];
        let mut acc = 0f32;
        for (k, h) in row.iter().enumerate() {
            let j = centre - self.half + k as i64;
            if j >= self.base && j < total {
                acc += self.held[(j - self.base) as usize] * h;
            }
        }
        out.push(acc);
    }

    /// Takes more input and writes every output sample it can now compute.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.ratio == 1.0 {
            out.extend_from_slice(input);
            return;
        }
        self.held.extend_from_slice(input);
        let total = self.base + self.held.len() as i64;
        loop {
            let at = self.next_out as f64 * self.ratio;
            if at.floor() as i64 + self.half >= total {
                break;
            }
            self.sample(at, total, out);
            self.next_out += 1;
        }
        let keep_from = (self.next_out as f64 * self.ratio).floor() as i64 - self.half;
        if keep_from > self.base {
            self.held.drain(..(keep_from - self.base) as usize);
            self.base = keep_from;
        }
    }

    /// The rest, once the input has ended, up to the length the input's duration gives.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.ratio == 1.0 {
            return;
        }
        let total = self.base + self.held.len() as i64;
        let wanted = (total as f64 / self.ratio).floor() as u64;
        while self.next_out < wanted {
            let at = self.next_out as f64 * self.ratio;
            self.sample(at, total, out);
            self.next_out += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &[f32], from: u32, to: u32, step: usize) -> Vec<f32> {
        let mut r = Resampler::new(from, to);
        let mut out = vec![];
        for part in input.chunks(step) {
            r.push(part, &mut out);
        }
        r.finish(&mut out);
        out
    }

    #[test]
    fn the_same_rate_passes_through() {
        assert_eq!(run(&[0.5, -0.5, 0.25], 16_000, 16_000, 2), vec![0.5, -0.5, 0.25]);
    }

    #[test]
    fn the_output_has_the_right_length_at_common_rates() {
        for from in [8_000, 22_050, 44_100, 48_000] {
            let input = vec![0.1f32; from as usize * 3];
            let out = run(&input, from, 16_000, 4096);
            assert!((out.len() as i64 - 48_000).abs() <= 1, "{from}: {}", out.len());
        }
    }

    #[test]
    fn how_the_input_is_cut_makes_no_difference() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.03).sin()).collect();
        let whole = run(&input, 48_000, 16_000, input.len());
        let pieces = run(&input, 48_000, 16_000, 777);
        assert_eq!(whole.len(), pieces.len());
        for (a, b) in whole.iter().zip(&pieces) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn a_tone_stays_a_tone_and_keeps_its_place() {
        // A click at exactly 2 s comes out within a sample or two of 2 s.
        let mut input = vec![0f32; 48_000 * 4];
        input[96_000] = 1.0;
        let out = run(&input, 48_000, 16_000, 4096);
        let peak = out.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
        assert!((peak as i64 - 32_000).abs() <= 2, "{peak}");
    }

    #[test]
    fn memory_holds_only_a_window() {
        let mut r = Resampler::new(48_000, 16_000);
        let mut out = vec![];
        for _ in 0..200 {
            r.push(&[0.0; 4800], &mut out);
            out.clear();
        }
        assert!(r.held.len() < 4800 + 2 * r.width, "{}", r.held.len());
    }
}
