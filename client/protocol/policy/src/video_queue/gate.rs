//! Display-pacing gate and recent wait-floor tracker for decoder debt recovery (Nightjar issue 7).

/// Whether the display, not the input, paces the decoder's outputs over a 10-second window.
#[derive(Debug, Clone)]
pub struct DisplayGate {
    totals: [i32; Self::BUCKETS],
    outputs: [i32; Self::BUCKETS],
    overs: [i32; Self::BUCKETS],
    behinds: [i32; Self::BUCKETS],
    sum_in: [f64; Self::BUCKETS],
    sum_out: [f64; Self::BUCKETS],
    sum_in_in: [f64; Self::BUCKETS],
    sum_in_out: [f64; Self::BUCKETS],
    sum_out_out: [f64; Self::BUCKETS],
    slot: usize,
    closed: i32,
    bucket_start_ns: i64,
    last_output_ns: i64,
    last_input_to_output_ns: i64,
    interval_ns: i64,
    spread_known: bool,
    slope: f64,
    held: bool,
}

impl DisplayGate {
    pub const BUCKETS: usize = 10;
    pub const BUCKET_NS: i64 = 1_000_000_000;
    pub const MAX_SLOPE: f64 = 0.3;
    pub const OVER_PERCENT: i32 = 80;
    pub const MAX_BEHIND_PERCENT: i32 = 10;
    pub const MIN_SPREAD_NS: i64 = 100_000;

    pub fn new() -> Self {
        Self {
            totals: [0; Self::BUCKETS],
            outputs: [0; Self::BUCKETS],
            overs: [0; Self::BUCKETS],
            behinds: [0; Self::BUCKETS],
            sum_in: [0.0; Self::BUCKETS],
            sum_out: [0.0; Self::BUCKETS],
            sum_in_in: [0.0; Self::BUCKETS],
            sum_in_out: [0.0; Self::BUCKETS],
            sum_out_out: [0.0; Self::BUCKETS],
            slot: 0,
            closed: 0,
            bucket_start_ns: 0,
            last_output_ns: 0,
            last_input_to_output_ns: 0,
            interval_ns: 0,
            spread_known: false,
            slope: 1.0,
            held: false,
        }
    }

    pub fn on_output(
        &mut self,
        input_to_output_ns: i64,
        output_ns: i64,
        content_interval_ns: i64,
        over: bool,
        behind: bool,
    ) {
        self.interval_ns = content_interval_ns;
        if self.bucket_start_ns == 0 {
            self.bucket_start_ns = output_ns;
        }
        let elapsed_ns = i128::from(output_ns) - i128::from(self.bucket_start_ns);
        if elapsed_ns >= i128::from(Self::BUCKET_NS) {
            let steps = elapsed_ns / i128::from(Self::BUCKET_NS);
            let close_steps = steps.min(Self::BUCKETS as i128);
            for _ in 0..close_steps {
                self.close();
            }
            self.bucket_start_ns =
                (i128::from(self.bucket_start_ns) + steps * i128::from(Self::BUCKET_NS))
                    .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        }
        self.totals[self.slot] += 1;
        if behind {
            self.behinds[self.slot] += 1;
        }
        if self.last_output_ns != 0 {
            let out_ns = i128::from(output_ns) - i128::from(self.last_output_ns);
            let in_ns = out_ns
                - (i128::from(input_to_output_ns) - i128::from(self.last_input_to_output_ns));
            let ci = i128::from(content_interval_ns);
            if (in_ns - ci).abs() * 2 < ci && (out_ns - ci).abs() * 2 < ci {
                let in_f = (in_ns - ci) as f64;
                let out_f = (out_ns - ci) as f64;
                self.outputs[self.slot] += 1;
                if over {
                    self.overs[self.slot] += 1;
                }
                self.sum_in[self.slot] += in_f;
                self.sum_out[self.slot] += out_f;
                self.sum_in_in[self.slot] += in_f * in_f;
                self.sum_in_out[self.slot] += in_f * out_f;
                self.sum_out_out[self.slot] += out_f * out_f;
            }
        }
        self.last_output_ns = output_ns;
        self.last_input_to_output_ns = input_to_output_ns;
    }

    pub fn held(&self) -> bool {
        self.held
    }

    pub fn still_held(&self, min_buckets: i32, max_slope: f64) -> bool {
        self.closed >= min_buckets && self.spread_known && self.slope <= max_slope
    }

    pub fn clear(&mut self) {
        self.totals.fill(0);
        self.outputs.fill(0);
        self.overs.fill(0);
        self.behinds.fill(0);
        self.sum_in.fill(0.0);
        self.sum_out.fill(0.0);
        self.sum_in_in.fill(0.0);
        self.sum_in_out.fill(0.0);
        self.sum_out_out.fill(0.0);
        self.slot = 0;
        self.closed = 0;
        self.bucket_start_ns = 0;
        self.spread_known = false;
        self.slope = 1.0;
        self.held = false;
    }

    pub fn reset(&mut self) {
        self.clear();
        self.last_output_ns = 0;
        self.last_input_to_output_ns = 0;
    }

    fn close(&mut self) {
        self.closed = (self.closed + 1).min(Self::BUCKETS as i32);
        let mut total = 0i32;
        let mut count = 0i32;
        let mut over = 0i32;
        let mut behind = 0i32;
        let mut sum_in = 0.0;
        let mut sum_out = 0.0;
        let mut sum_in_in = 0.0;
        let mut sum_in_out = 0.0;
        let mut sum_out_out = 0.0;
        for i in 0..Self::BUCKETS {
            total += self.totals[i];
            count += self.outputs[i];
            over += self.overs[i];
            behind += self.behinds[i];
            sum_in += self.sum_in[i];
            sum_out += self.sum_out[i];
            sum_in_in += self.sum_in_in[i];
            sum_in_out += self.sum_in_out[i];
            sum_out_out += self.sum_out_out[i];
        }
        let count_f = f64::from(count);
        let variance_ns2 = if count > 2 {
            sum_in_in - sum_in * sum_in / count_f
        } else {
            0.0
        };
        let min_spread = Self::MIN_SPREAD_NS as f64;
        self.spread_known = count > 2 && variance_ns2 >= count_f * min_spread * min_spread;
        let covariance = if self.spread_known {
            sum_in_out - sum_in * sum_out / count_f
        } else {
            0.0
        };
        self.slope = if self.spread_known {
            covariance / variance_ns2
        } else {
            1.0
        };
        let slope_error = if self.spread_known {
            let resid =
                (sum_out_out - sum_out * sum_out / count_f - self.slope * covariance).max(0.0);
            (resid / (f64::from(count - 2) * variance_ns2)).sqrt()
        } else {
            0.0
        };
        self.held = self.closed == Self::BUCKETS as i32
            && self.spread_known
            && self.slope + 2.0 * slope_error <= Self::MAX_SLOPE
            && i128::from(count) * i128::from(self.interval_ns) * 2
                >= i128::from(Self::BUCKETS as i64) * i128::from(Self::BUCKET_NS)
            && i128::from(over) * 100 >= i128::from(count) * i128::from(Self::OVER_PERCENT)
            && i128::from(self.overs[self.slot]) * 100
                >= i128::from(self.outputs[self.slot]) * i128::from(Self::OVER_PERCENT)
            && self.outputs[self.slot] > 0
            && i128::from(behind) * 100 <= i128::from(total) * i128::from(Self::MAX_BEHIND_PERCENT);

        self.slot = (self.slot + 1) % Self::BUCKETS;
        self.totals[self.slot] = 0;
        self.outputs[self.slot] = 0;
        self.overs[self.slot] = 0;
        self.behinds[self.slot] = 0;
        self.sum_in[self.slot] = 0.0;
        self.sum_out[self.slot] = 0.0;
        self.sum_in_in[self.slot] = 0.0;
        self.sum_in_out[self.slot] = 0.0;
        self.sum_out_out[self.slot] = 0.0;
    }
}

impl Default for DisplayGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Low percentile of wait times over the last minute of outputs (20th percentile per second,
/// 10th percentile across the last 60 seconds).
#[derive(Debug, Clone)]
pub struct RecentLevel {
    samples: [i64; 256],
    levels: [i64; Self::SECONDS],
    sample_count: usize,
    level_count: usize,
    slot: usize,
    second_start_ns: i64,
    last_ns: i64,
    low_ns: i64,
}

impl RecentLevel {
    pub const SECONDS: usize = 60;
    pub const SECOND_NS: i64 = 1_000_000_000;
    const SECOND_PERCENT: usize = 20;
    const SECONDS_PERCENT: usize = 10;

    pub fn new() -> Self {
        Self {
            samples: [0; 256],
            levels: [0; Self::SECONDS],
            sample_count: 0,
            level_count: 0,
            slot: 0,
            second_start_ns: 0,
            last_ns: 0,
            low_ns: i64::MAX,
        }
    }

    pub fn add(&mut self, value_ns: i64, now_ns: i64) {
        if self.second_start_ns == 0 {
            self.second_start_ns = now_ns;
        }
        if i128::from(now_ns) - i128::from(self.second_start_ns) >= i128::from(Self::SECOND_NS) {
            self.close();
            self.second_start_ns = now_ns;
        }
        if self.sample_count < self.samples.len() {
            self.samples[self.sample_count] = value_ns;
            self.sample_count += 1;
        }
    }

    pub fn low_ns(&self) -> i64 {
        self.low_ns
    }

    pub fn last_ns(&self) -> i64 {
        self.last_ns
    }

    pub fn reset(&mut self) {
        self.sample_count = 0;
        self.level_count = 0;
        self.slot = 0;
        self.second_start_ns = 0;
        self.last_ns = 0;
        self.low_ns = i64::MAX;
    }

    fn close(&mut self) {
        self.samples[..self.sample_count].sort_unstable();
        self.last_ns = self.samples[self.sample_count * Self::SECOND_PERCENT / 100];
        self.sample_count = 0;
        self.levels[self.slot] = self.last_ns;
        self.slot = (self.slot + 1) % Self::SECONDS;
        self.level_count = (self.level_count + 1).min(Self::SECONDS);
        let mut sorted = [0i64; Self::SECONDS];
        sorted[..self.level_count].copy_from_slice(&self.levels[..self.level_count]);
        sorted[..self.level_count].sort_unstable();
        self.low_ns = sorted[self.level_count * Self::SECONDS_PERCENT / 100];
    }
}

impl Default for RecentLevel {
    fn default() -> Self {
        Self::new()
    }
}
