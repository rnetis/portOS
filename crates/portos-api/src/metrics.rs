//! Minimal Prometheus text-exposition metrics: counters, gauges and
//! histograms (PRD §6.5 observability).

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct Metrics {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    counters: HashMap<String, u64>,
    gauges: HashMap<String, f64>,
    histograms: HashMap<String, Histogram>,
}

#[derive(Default)]
struct Histogram {
    buckets: Vec<(f64, u64)>, // (upper bound, cumulative count)
    count: u64,
    sum: f64,
}

const DEFAULT_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

impl Metrics {
    pub fn inc(&self, name: &str, by: u64) {
        if let Ok(mut g) = self.inner.lock() {
            *g.counters.entry(name.to_string()).or_insert(0) += by;
        }
    }

    pub fn inc_by_one(&self, name: &str) {
        self.inc(name, 1);
    }

    pub fn set_gauge(&self, name: &str, value: f64) {
        if let Ok(mut g) = self.inner.lock() {
            g.gauges.insert(name.to_string(), value);
        }
    }

    pub fn observe(&self, name: &str, value: f64) {
        if let Ok(mut g) = self.inner.lock() {
            let h = g
                .histograms
                .entry(name.to_string())
                .or_insert_with(|| Histogram {
                    buckets: DEFAULT_BUCKETS.iter().map(|b| (*b, 0)).collect(),
                    count: 0,
                    sum: 0.0,
                });
            h.count += 1;
            h.sum += value;
            for (bound, count) in h.buckets.iter_mut() {
                if value <= *bound {
                    *count += 1;
                }
            }
        }
    }

    /// Prometheus text format (0.0.4).
    #[must_use]
    pub fn render(&self) -> String {
        let g = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return String::new(),
        };
        let mut out = String::new();
        let mut keys: Vec<&String> = g.counters.keys().collect();
        keys.sort();
        for k in keys {
            out.push_str(&format!("# TYPE {k} counter\n{k} {}\n", g.counters[k]));
        }
        let mut keys: Vec<&String> = g.gauges.keys().collect();
        keys.sort();
        for k in keys {
            out.push_str(&format!("# TYPE {k} gauge\n{k} {}\n", g.gauges[k]));
        }
        let mut keys: Vec<&String> = g.histograms.keys().collect();
        keys.sort();
        for k in keys {
            let h = &g.histograms[k];
            out.push_str(&format!("# TYPE {k} histogram\n"));
            for (bound, count) in &h.buckets {
                out.push_str(&format!("{k}_bucket{{le=\"{bound}\"}} {count}\n"));
            }
            out.push_str(&format!("{k}_bucket{{le=\"+Inf\"}} {}\n", h.count));
            out.push_str(&format!("{k}_count {}\n", h.count));
            out.push_str(&format!("{k}_sum {}\n", h.sum));
        }
        out
    }
}

/// Global metrics handle (avoids threading through every layer).
pub static METRICS: std::sync::LazyLock<Metrics> = std::sync::LazyLock::new(Metrics::default);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_gauges_histograms_render() {
        let m = Metrics::default();
        m.inc_by_one("portos_gate_tx_total");
        m.set_gauge("portos_yard_occupancy", 63.5);
        m.observe("portos_http_duration_seconds", 0.42);
        let text = m.render();
        assert!(text.contains("portos_gate_tx_total 1"));
        assert!(text.contains("portos_yard_occupancy 63.5"));
        assert!(text.contains("portos_http_duration_seconds_bucket{le=\"0.5\"} 1"));
        assert!(text.contains("portos_http_duration_seconds_sum 0.42"));
    }
}
